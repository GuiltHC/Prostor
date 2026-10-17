using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

public sealed class DuplicateFile : ObservableObject
{
    public string Path { get; init; }
    public string Name => System.IO.Path.GetFileName(Path);
    public string Folder => System.IO.Path.GetDirectoryName(Path);
    public long LastWrite { get; init; }
    public string DateText => LastWrite > 0 ? Fmt.Date(DateTime.FromFileTime(LastWrite)) : "—";
    public DuplicateGroup Group { get; set; }

    private bool _isChecked;
    public bool IsChecked
    {
        get => _isChecked;
        set
        {
            if (Set(ref _isChecked, value)) Group?.RaiseChecked();
        }
    }
}

public sealed class DuplicateGroup : ObservableObject
{
    public long Size { get; init; }
    public List<DuplicateFile> Files { get; init; }
    public string Name => Files.Count > 0 ? Files[0].Name : "";
    public long Wasted => Size * (Files.Count - 1);
    public string SizeText => Fmt.Size(Size);
    public string Summary => $"{Files.Count} {Fmt.Plural(Files.Count, "копия", "копии", "копий")} по {Fmt.Size(Size)} · лишние {Fmt.Size(Wasted)}";
    public event Action CheckedChanged;
    internal void RaiseChecked() => CheckedChanged?.Invoke();
}

/// <summary>Finds identical files: same size → same first/last 64 KB → same SHA-256 → not the same file (hard link).</summary>
public sealed class DuplicateFinder
{
    private const int Edge = 64 * 1024;

    public long CandidateBytes { get; private set; }
    public int GroupsTotal { get; private set; }
    private int _groupsDone;
    public int GroupsDone => Volatile.Read(ref _groupsDone);
    private long _hashed;
    public long HashedBytes => Interlocked.Read(ref _hashed);
    public string Stage { get; private set; } = "";

    /// <param name="skip">Folders that must not be touched (program folders, Windows, …).</param>
    public List<DuplicateGroup> Find(DirNode root, long minSize, Func<string, bool> skip, CancellationToken ct)
    {
        // 1. group by size
        Stage = "Сравниваем размеры…";
        var bySize = new Dictionary<long, List<string>>();
        void Walk(DirNode node, string path)
        {
            ct.ThrowIfCancellationRequested();
            foreach (var f in node.Files)
            {
                if (f.Size < minSize) break; // files are sorted by size, largest first
                if (!bySize.TryGetValue(f.Size, out var list)) bySize[f.Size] = list = new List<string>(2);
                list.Add(FastDir.Combine(path, f.Name));
            }
            foreach (var d in node.Dirs)
            {
                if (d.IsVirtual) continue;
                string sub = FastDir.Combine(path, d.Name);
                if (!skip(sub)) Walk(d, sub);
            }
        }
        Walk(root, root.FullPath);
        var candidates = bySize.Where(kv => kv.Value.Count > 1).ToList();
        CandidateBytes = candidates.Sum(kv => kv.Key * kv.Value.Count);
        GroupsTotal = candidates.Count;

        // 2. quick hash of the beginning and the end, 3. full hash
        var groups = new List<DuplicateGroup>();
        Stage = "Сравниваем содержимое…";
        var options = new ParallelOptions { MaxDegreeOfParallelism = 4, CancellationToken = ct };
        var bag = new System.Collections.Concurrent.ConcurrentBag<DuplicateGroup>();
        Parallel.ForEach(candidates, options, kv =>
        {
            long size = kv.Key;
            var quick = kv.Value.GroupBy(p => QuickHash(p, size), StringComparer.Ordinal).Where(g => g.Key != null && g.Count() > 1);
            foreach (var q in quick)
            {
                var full = size <= 2 * Edge
                    ? new[] { q.ToList() }
                    : q.GroupBy(p => FullHash(p, ct), StringComparer.Ordinal).Where(g => g.Key != null && g.Count() > 1).Select(g => g.ToList()).ToArray();
                foreach (var same in full)
                {
                    var unique = WithoutHardLinks(same);
                    if (unique.Count < 2) continue;
                    var files = unique.Select(p => new DuplicateFile { Path = p, LastWrite = LastWrite(p) }).ToList();
                    var group = new DuplicateGroup { Size = size, Files = files };
                    foreach (var f in files) f.Group = group;
                    bag.Add(group);
                }
            }
            Interlocked.Increment(ref _groupsDone);
        });
        groups.AddRange(bag);
        return groups.OrderByDescending(g => g.Wasted).ToList();
    }

    private string QuickHash(string path, long size)
    {
        try
        {
            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 1, FileOptions.RandomAccess);
            var buf = new byte[size <= 2 * Edge ? size : 2 * Edge];
            int n = fs.ReadAtLeast(buf.AsSpan(0, (int)Math.Min(Edge, size)), (int)Math.Min(Edge, size), false);
            if (size > 2 * Edge)
            {
                fs.Seek(-Edge, SeekOrigin.End);
                n += fs.ReadAtLeast(buf.AsSpan(Edge, Edge), Edge, false);
            }
            else if (size > Edge)
            {
                n += fs.ReadAtLeast(buf.AsSpan(Edge, (int)(size - Edge)), (int)(size - Edge), false);
            }
            Interlocked.Add(ref _hashed, n);
            return Convert.ToHexString(SHA256.HashData(buf.AsSpan(0, n)));
        }
        catch
        {
            return null;
        }
    }

    private string FullHash(string path, CancellationToken ct)
    {
        try
        {
            using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 1 << 20, FileOptions.SequentialScan);
            using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
            var buffer = new byte[1 << 20];
            int n;
            while ((n = fs.Read(buffer, 0, buffer.Length)) > 0)
            {
                ct.ThrowIfCancellationRequested();
                sha.AppendData(buffer, 0, n);
                Interlocked.Add(ref _hashed, n);
            }
            return Convert.ToHexString(sha.GetHashAndReset());
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>Hard links point to the same data: they are not real copies.</summary>
    private static List<string> WithoutHardLinks(List<string> paths)
    {
        var seen = new HashSet<(uint, uint, uint)>();
        var result = new List<string>();
        foreach (var p in paths)
        {
            try
            {
                using SafeFileHandle h = File.OpenHandle(p, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                if (Native.GetFileInformationByHandle(h, out var info) &&
                    !seen.Add((info.VolumeSerialNumber, info.FileIndexHigh, info.FileIndexLow)))
                    continue;
            }
            catch { }
            result.Add(p);
        }
        return result;
    }

    private static long LastWrite(string path)
    {
        try { return File.GetLastWriteTime(path).ToFileTime(); }
        catch { return 0; }
    }

    /// <summary>Folders that duplicates are never searched in: Windows, program folders, caches of the system.</summary>
    public static Func<string, bool> DefaultSkip(IEnumerable<string> programFolders)
    {
        var roots = new List<string>
        {
            Environment.GetFolderPath(Environment.SpecialFolder.Windows),
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), @"AppData\LocalLow"),
        };
        roots.AddRange(programFolders);
        roots.AddRange(AppSettings.Current.Exclusions);
        var set = roots.Where(r => !string.IsNullOrWhiteSpace(r)).Select(r => r.TrimEnd('\\')).Distinct(StringComparer.OrdinalIgnoreCase).ToList();
        return path =>
        {
            string name = Path.GetFileName(path);
            if (name is "$Recycle.Bin" or "System Volume Information" or "$WinREAgent" or "Recovery" or "node_modules" or ".git" or "WindowsApps" or "XboxGames")
                return true;
            foreach (var r in set)
                if (path.Equals(r, StringComparison.OrdinalIgnoreCase) || path.StartsWith(r + "\\", StringComparison.OrdinalIgnoreCase))
                    return true;
            return false;
        };
    }
}
