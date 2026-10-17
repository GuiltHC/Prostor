using System.Runtime.InteropServices;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

/// <summary>Builds a size tree of a folder or drive. Junctions and symbolic links are not followed.</summary>
public sealed class DiskScanner
{
    private long _files, _dirs, _bytes;
    private volatile string _current = "";
    private CancellationToken _ct;
    private readonly int _parallelism = Math.Max(2, Environment.ProcessorCount);

    public long FilesScanned => Interlocked.Read(ref _files);
    public long DirsScanned => Interlocked.Read(ref _dirs);
    public long BytesScanned => Interlocked.Read(ref _bytes);
    public string CurrentPath => _current;

    public DirNode Scan(string rootPath, CancellationToken ct)
    {
        _ct = ct;
        string root = rootPath.Length <= 3 ? rootPath.TrimEnd('\\') + "\\" : rootPath.TrimEnd('\\');
        var node = new DirNode { Name = root };
        ScanDir(node, root, 0);
        ct.ThrowIfCancellationRequested();
        node.CategorySizes = null;
        return node;
    }

    private unsafe void ScanDir(DirNode node, string path, int depth)
    {
        if (_ct.IsCancellationRequested) return;

        List<FileEntry> files = null;
        List<(DirNode node, string path)> subdirs = null;
        long localBytes = 0;

        IntPtr h = Native.FindFirstFileEx(FastDir.ToSearchPattern(path), FastDir.FindExInfoBasic, out var d, 0, IntPtr.Zero,
            FastDir.FIND_FIRST_EX_LARGE_FETCH);
        if (h == Native.INVALID_HANDLE_VALUE)
        {
            int err = Marshal.GetLastWin32Error();
            if (err == 5) node.AccessDenied = true;
            Interlocked.Increment(ref _dirs);
            return;
        }

        try
        {
            do
            {
                char* n = d.cFileName;
                if (n[0] == '.' && (n[1] == '\0' || (n[1] == '.' && n[2] == '\0'))) continue;
                uint attr = d.dwFileAttributes;
                long ft = ((long)d.ftLastWriteTimeHigh << 32) | d.ftLastWriteTimeLow;

                if ((attr & Native.FILE_ATTRIBUTE_DIRECTORY) != 0)
                {
                    if ((attr & Native.FILE_ATTRIBUTE_REPARSE_POINT) != 0 &&
                        (d.dwReserved0 == Native.IO_REPARSE_TAG_MOUNT_POINT || d.dwReserved0 == Native.IO_REPARSE_TAG_SYMLINK))
                        continue;
                    string name = new string(n);
                    var child = new DirNode { Name = name, Parent = node, LastWrite = ft };
                    (subdirs ??= new List<(DirNode, string)>()).Add((child, FastDir.Combine(path, name)));
                }
                else
                {
                    long size = ((long)d.nFileSizeHigh << 32) | d.nFileSizeLow;
                    if ((attr & Native.FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0) size = 0; // cloud-only file
                    string name = new string(n);
                    long access = ((long)d.ftLastAccessTimeHigh << 32) | d.ftLastAccessTimeLow;
                    long created = ((long)d.ftCreationTimeHigh << 32) | d.ftCreationTimeLow;
                    (files ??= new List<FileEntry>()).Add(new FileEntry
                    {
                        Name = name,
                        Size = size,
                        LastWrite = ft,
                        LastUsed = Math.Max(ft, access),
                        Created = created,
                        Category = (byte)FileCategories.Of(name)
                    });
                    localBytes += size;
                }
            } while (Native.FindNextFile(h, out d));
        }
        finally
        {
            Native.FindClose(h);
        }

        int fileCount = files?.Count ?? 0;
        Interlocked.Add(ref _files, fileCount);
        Interlocked.Add(ref _bytes, localBytes);
        Interlocked.Increment(ref _dirs);
        _current = path;

        var cats = new long[FileCategories.All.Length];
        if (files != null)
        {
            var arr = files.ToArray();
            Array.Sort(arr, static (a, b) => b.Size.CompareTo(a.Size));
            node.Files = arr;
            foreach (var f in arr) cats[f.Category] += f.Size;
        }

        long size2 = localBytes, fc = fileCount, dc = 0;
        if (subdirs != null)
        {
            if (depth < 5 && subdirs.Count > 1)
            {
                Parallel.ForEach(subdirs, new ParallelOptions { MaxDegreeOfParallelism = _parallelism },
                    s => ScanDir(s.node, s.path, depth + 1));
            }
            else
            {
                foreach (var s in subdirs) ScanDir(s.node, s.path, depth + 1);
            }

            var list = new List<DirNode>(subdirs.Count);
            foreach (var s in subdirs)
            {
                list.Add(s.node);
                size2 += s.node.Size;
                fc += s.node.FileCount;
                dc += s.node.DirCount + 1;
                if (s.node.CategorySizes != null)
                {
                    for (int i = 0; i < cats.Length; i++) cats[i] += s.node.CategorySizes[i];
                    s.node.CategorySizes = null;
                }
            }
            list.Sort(static (a, b) => b.Size.CompareTo(a.Size));
            node.Dirs = list;
        }

        node.Size = size2;
        node.FileCount = fc;
        node.DirCount = dc;
        node.CategorySizes = cats;
        int best = (int)FileCategory.Other;
        for (int i = 0; i < cats.Length; i++)
            if (cats[i] > cats[best]) best = i;
        node.Dominant = (byte)best;
    }
}

public sealed class LargeFile
{
    public DirNode Parent { get; init; }
    public string Name { get; init; }
    public long Size { get; init; }
    public long LastWrite { get; init; }
    public long LastUsed { get; init; }
    public long Created { get; init; }
    public byte Category { get; init; }

    public string FullPath => Path.Combine(Parent.FullPath, Name);
    public string UsedText => LastUsed > 0 ? "открывался " + Fmt.Ago(DateTime.FromFileTime(LastUsed)) : "—";
    public string CreatedText => Created > 0 ? "скачан " + Fmt.Ago(DateTime.FromFileTime(Created)) : "—";
    public string FolderPath => Parent.FullPath;
    public string SizeText => Fmt.Size(Size);
    public string DateText => LastWrite > 0 ? Fmt.Date(DateTime.FromFileTime(LastWrite)) : "—";
    public CategoryInfo Info => FileCategories.Get(Category);
    public FileRef Ref => new(Parent, Name);
}

public sealed class CategoryStat
{
    public CategoryInfo Info { get; init; }
    public long Size { get; init; }
    public long Count { get; init; }
    public double Fraction { get; init; }
    public string SizeText => Fmt.Size(Size);
    public string CountText => Fmt.Files(Count);
    public string PercentText => Fmt.Percent(Fraction);
}

public sealed class ExtStat
{
    public string Ext { get; init; }
    public long Size { get; init; }
    public long Count { get; init; }
    public double Fraction { get; init; }
    public CategoryInfo Info { get; init; }
    public string SizeText => Fmt.Size(Size);
    public string CountText => Fmt.Files(Count);
}

public sealed class ScanStats
{
    public List<LargeFile> LargestFiles { get; init; }
    /// <summary>Big files not opened or changed for more than a year.</summary>
    public List<LargeFile> OldFiles { get; init; }
    /// <summary>Big files that have been lying in Downloads for more than three months.</summary>
    public List<LargeFile> OldDownloads { get; init; }
    public long OldFilesSize { get; init; }
    public long OldDownloadsSize { get; init; }
    public List<CategoryStat> Categories { get; init; }
    public List<ExtStat> Extensions { get; init; }

    private sealed class Acc
    {
        public long Count, Size;
    }

    public static ScanStats Compute(DirNode root, int topCount = 300)
    {
        var heap = new PriorityQueue<(DirNode parent, FileEntry file), long>();
        var oldHeap = new PriorityQueue<(DirNode parent, FileEntry file), long>();
        var dlHeap = new PriorityQueue<(DirNode parent, FileEntry file), long>();
        long oldSize = 0, dlSize = 0;
        var catSize = new long[FileCategories.All.Length];
        var catCount = new long[FileCategories.All.Length];
        var exts = new Dictionary<string, Acc>(StringComparer.OrdinalIgnoreCase);
        var lookup = exts.GetAlternateLookup<ReadOnlySpan<char>>();

        long yearAgo = DateTime.Now.AddYears(-1).ToFileTime();
        long quarterAgo = DateTime.Now.AddDays(-90).ToFileTime();
        const long minOld = 10L << 20;
        var downloads = FindNode(root, ShellHelper.DownloadsFolder);

        static void Push(PriorityQueue<(DirNode, FileEntry), long> h, DirNode dir, FileEntry f, int top)
        {
            if (h.Count < top) h.Enqueue((dir, f), f.Size);
            else if (h.TryPeek(out _, out long min) && f.Size > min) h.EnqueueDequeue((dir, f), f.Size);
        }

        // system and program folders: their old files are still needed (installers cache, program data)
        var systemDirs = new HashSet<DirNode>(new[]
        {
            Environment.GetFolderPath(Environment.SpecialFolder.Windows),
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
            Path.Combine(Path.GetPathRoot(Environment.SystemDirectory) ?? "C:\\", "System Volume Information"),
            Path.Combine(Path.GetPathRoot(Environment.SystemDirectory) ?? "C:\\", "$Recycle.Bin"),
            Path.Combine(Path.GetPathRoot(Environment.SystemDirectory) ?? "C:\\", "Recovery"),
        }.Select(p => FindNode(root, p)).Where(n => n != null && n != root));

        var stack = new Stack<(DirNode node, bool inDownloads, bool system)>();
        stack.Push((root, root == downloads, false));
        while (stack.Count > 0)
        {
            var (dir, inDl, sys) = stack.Pop();
            foreach (var d in dir.Dirs)
                if (!d.IsVirtual) stack.Push((d, inDl || d == downloads, sys || systemDirs.Contains(d)));
            bool isRoot = dir.Parent == null && dir.Name.Length <= 3;

            foreach (var f in dir.Files)
            {
                catSize[f.Category] += f.Size;
                catCount[f.Category]++;

                var ext = FileCategories.Extension(f.Name);
                if (!ext.IsEmpty)
                {
                    if (!lookup.TryGetValue(ext, out var acc))
                    {
                        acc = new Acc();
                        exts[ext.ToString().ToLowerInvariant()] = acc;
                    }
                    acc.Count++;
                    acc.Size += f.Size;
                }

                Push(heap, dir, f, topCount);
                if (f.Size >= minOld && !sys && !(isRoot && (f.Name.StartsWith('$') || f.Name.EndsWith(".sys", StringComparison.OrdinalIgnoreCase))))
                {
                    long used = f.LastUsed > 0 ? f.LastUsed : f.LastWrite;
                    if (used > 0 && used < yearAgo)
                    {
                        Push(oldHeap, dir, f, topCount);
                        oldSize += f.Size;
                    }
                    long arrived = f.Created > 0 ? f.Created : f.LastWrite;
                    if (inDl && arrived > 0 && arrived < quarterAgo)
                    {
                        Push(dlHeap, dir, f, topCount);
                        dlSize += f.Size;
                    }
                }
            }
        }

        static List<LargeFile> Drain(PriorityQueue<(DirNode parent, FileEntry file), long> h)
        {
            var list = new List<LargeFile>(h.Count);
            while (h.TryDequeue(out var item, out _))
                list.Add(new LargeFile
                {
                    Parent = item.parent,
                    Name = item.file.Name,
                    Size = item.file.Size,
                    LastWrite = item.file.LastWrite,
                    LastUsed = item.file.LastUsed,
                    Created = item.file.Created,
                    Category = item.file.Category
                });
            list.Reverse();
            return list;
        }

        long total = Math.Max(1, catSize.Sum());
        var cats = FileCategories.All
            .Select((info, i) => new CategoryStat
            {
                Info = info,
                Size = catSize[i],
                Count = catCount[i],
                Fraction = (double)catSize[i] / total
            })
            .Where(c => c.Count > 0)
            .OrderByDescending(c => c.Size)
            .ToList();

        var topExts = exts.OrderByDescending(e => e.Value.Size).Take(40).ToList();
        long maxExt = topExts.Count > 0 ? Math.Max(1, topExts[0].Value.Size) : 1;
        var extStats = topExts.Select(e => new ExtStat
        {
            Ext = "." + e.Key,
            Size = e.Value.Size,
            Count = e.Value.Count,
            Fraction = (double)e.Value.Size / maxExt,
            Info = FileCategories.Get(FileCategories.Of("x." + e.Key))
        }).ToList();

        return new ScanStats
        {
            LargestFiles = Drain(heap), OldFiles = Drain(oldHeap), OldDownloads = Drain(dlHeap),
            OldFilesSize = oldSize, OldDownloadsSize = dlSize, Categories = cats, Extensions = extStats
        };
    }

    /// <summary>The node of <paramref name="path"/> inside the scanned tree, or null.</summary>
    public static DirNode FindNode(DirNode root, string path)
    {
        if (root == null || string.IsNullOrEmpty(path)) return null;
        string rootPath = root.FullPath.TrimEnd('\\');
        string full = path.TrimEnd('\\');
        if (full.Equals(rootPath, StringComparison.OrdinalIgnoreCase)) return root;
        if (!full.StartsWith(rootPath + "\\", StringComparison.OrdinalIgnoreCase)) return null;
        var node = root;
        foreach (var part in full.Substring(rootPath.Length + 1).Split('\\', StringSplitOptions.RemoveEmptyEntries))
        {
            node = node.Dirs.FirstOrDefault(d => d.Name.Equals(part, StringComparison.OrdinalIgnoreCase));
            if (node == null) return null;
        }
        return node;
    }
}
