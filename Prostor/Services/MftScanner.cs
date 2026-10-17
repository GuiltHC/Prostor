using System.Buffers.Binary;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

/// <summary>
/// Reads the NTFS Master File Table directly (like WizTree): the whole volume in seconds instead of walking every folder.
/// Needs administrator rights. Hard links are counted once, files of other users and system folders are included.
/// </summary>
public sealed class MftScanner
{
    private const uint GENERIC_READ = 0x80000000;
    private const uint FILE_SHARE_ALL = 0x7;
    private const uint OPEN_EXISTING = 3;
    private const int RootFrn = 5;

    private long _records, _total, _bytes;
    private volatile string _current = "";

    public long RecordsRead => Interlocked.Read(ref _records);
    public long RecordsTotal => Interlocked.Read(ref _total);
    public long BytesScanned => Interlocked.Read(ref _bytes);
    public string CurrentPath => _current;

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);

    /// <summary>MFT reading is possible: administrator, local NTFS volume.</summary>
    public static bool CanUse(string path)
    {
        if (!ShellHelper.IsAdmin) return false;
        try
        {
            string root = Path.GetPathRoot(Path.GetFullPath(path));
            if (root == null || root.Length != 3 || root[1] != ':') return false;
            var d = new DriveInfo(root);
            return d.IsReady && d.DriveType == DriveType.Fixed && d.DriveFormat == "NTFS";
        }
        catch
        {
            return false;
        }
    }

    // per record (index = file reference number)
    private int[] _parent;
    private string[] _name;
    private long[] _size;
    private long[] _write;
    private long[] _used;
    private long[] _created;
    private byte[] _flags; // 1 = in use, 2 = directory, 4 = has name, 8 = link (junction/symlink), 16 = cloud placeholder

    public DirNode Scan(string path, CancellationToken ct)
    {
        string full = Path.GetFullPath(path);
        string root = Path.GetPathRoot(full)!;
        using var volume = CreateFile(@"\\.\" + root.TrimEnd('\\'), GENERIC_READ, FILE_SHARE_ALL, IntPtr.Zero, OPEN_EXISTING, 0, IntPtr.Zero);
        if (volume.IsInvalid) throw new UnauthorizedAccessException("Нет доступа к тому " + root);

        // ---- boot sector
        var boot = new byte[4096];
        ReadExact(volume, boot, 0);
        if (System.Text.Encoding.ASCII.GetString(boot, 3, 4) != "NTFS") throw new InvalidOperationException("Это не NTFS");
        int bytesPerSector = BinaryPrimitives.ReadUInt16LittleEndian(boot.AsSpan(0x0B));
        int spcRaw = boot[0x0D];
        int sectorsPerCluster = spcRaw <= 0x80 ? spcRaw : 1 << (256 - spcRaw);
        long clusterSize = (long)bytesPerSector * sectorsPerCluster;
        long mftLcn = BinaryPrimitives.ReadInt64LittleEndian(boot.AsSpan(0x30));
        sbyte cpr = (sbyte)boot[0x40];
        int recordSize = cpr > 0 ? (int)(cpr * clusterSize) : 1 << -cpr;

        // ---- $MFT record 0 → where the MFT lives
        var rec0 = new byte[Align(recordSize, 4096)];
        ReadExact(volume, rec0, mftLcn * clusterSize);
        if (!ApplyFixups(rec0.AsSpan(0, recordSize))) throw new InvalidOperationException("Повреждена запись $MFT");
        var (runs, mftSize) = MftDataRuns(rec0.AsSpan(0, recordSize));
        if (runs.Count == 0 || mftSize <= 0) throw new InvalidOperationException("Не найдено расположение MFT");

        long count = mftSize / recordSize;
        if (count > int.MaxValue) throw new InvalidOperationException("Слишком большой том");
        Interlocked.Exchange(ref _total, count);
        Allocate((int)count);

        // ---- read all records extent by extent
        const int chunk = 4 << 20;
        var buffer = new byte[chunk];
        long frn = 0;
        foreach (var (lcn, clusters) in runs)
        {
            long offset = lcn * clusterSize;
            long remaining = clusters * clusterSize;
            while (remaining > 0 && frn < count)
            {
                ct.ThrowIfCancellationRequested();
                int len = (int)Math.Min(chunk, remaining);
                len = Align(len, bytesPerSector);
                ReadExact(volume, buffer.AsSpan(0, len), offset);
                for (int pos = 0; pos + recordSize <= len && frn < count; pos += recordSize, frn++)
                    ParseRecord(buffer.AsSpan(pos, recordSize), (int)frn);
                Interlocked.Exchange(ref _records, frn);
                offset += len;
                remaining -= len;
            }
        }
        _current = "Строим дерево папок…";
        return BuildTree(root, full, ct);
    }

    private void Allocate(int n)
    {
        _parent = new int[n];
        _name = new string[n];
        _size = new long[n];
        _write = new long[n];
        _used = new long[n];
        _created = new long[n];
        _flags = new byte[n];
    }

    private static int Align(long value, int to) => (int)((value + to - 1) / to * to);

    private static void ReadExact(SafeFileHandle h, Span<byte> buffer, long offset)
    {
        int done = 0;
        while (done < buffer.Length)
        {
            int n = RandomAccess.Read(h, buffer[done..], offset + done);
            if (n <= 0) throw new EndOfStreamException();
            done += n;
        }
    }

    /// <summary>Restores the last two bytes of every 512-byte block from the update sequence array.</summary>
    public static bool ApplyFixups(Span<byte> rec)
    {
        if (rec.Length < 48 || rec[0] != (byte)'F' || rec[1] != (byte)'I' || rec[2] != (byte)'L' || rec[3] != (byte)'E') return false;
        int usaOffset = BinaryPrimitives.ReadUInt16LittleEndian(rec[4..]);
        int usaCount = BinaryPrimitives.ReadUInt16LittleEndian(rec[6..]);
        if (usaCount < 2 || usaOffset + usaCount * 2 > rec.Length) return false;
        int stride = rec.Length / (usaCount - 1);
        ushort usn = BinaryPrimitives.ReadUInt16LittleEndian(rec[usaOffset..]);
        for (int i = 1; i < usaCount; i++)
        {
            int end = i * stride - 2;
            if (end + 2 > rec.Length) return false;
            if (BinaryPrimitives.ReadUInt16LittleEndian(rec[end..]) != usn) return false; // torn write
            rec[end] = rec[usaOffset + i * 2];
            rec[end + 1] = rec[usaOffset + i * 2 + 1];
        }
        return true;
    }

    private static (List<(long lcn, long clusters)> runs, long size) MftDataRuns(Span<byte> rec)
    {
        int off = BinaryPrimitives.ReadUInt16LittleEndian(rec[0x14..]);
        while (off + 16 <= rec.Length)
        {
            uint type = BinaryPrimitives.ReadUInt32LittleEndian(rec[off..]);
            if (type == 0xFFFFFFFF) break;
            int len = (int)BinaryPrimitives.ReadUInt32LittleEndian(rec[(off + 4)..]);
            if (len <= 0 || off + len > rec.Length) break;
            if (type == 0x80 && rec[off + 8] == 1 && rec[off + 9] == 0)
            {
                long size = BinaryPrimitives.ReadInt64LittleEndian(rec[(off + 0x30)..]);
                int runOff = BinaryPrimitives.ReadUInt16LittleEndian(rec[(off + 0x20)..]);
                return (ParseRuns(rec.Slice(off + runOff, len - runOff)), size);
            }
            off += len;
        }
        return (new List<(long, long)>(), 0);
    }

    public static List<(long lcn, long clusters)> ParseRuns(ReadOnlySpan<byte> data)
    {
        var runs = new List<(long, long)>();
        long lcn = 0;
        int i = 0;
        while (i < data.Length && data[i] != 0)
        {
            int header = data[i++];
            int lenSize = header & 0x0F, offSize = header >> 4;
            if (lenSize == 0 || i + lenSize + offSize > data.Length) break;
            long length = 0;
            for (int k = 0; k < lenSize; k++) length |= (long)data[i + k] << (8 * k);
            i += lenSize;
            if (offSize == 0)
            {
                continue; // sparse run
            }
            long delta = 0;
            for (int k = 0; k < offSize; k++) delta |= (long)data[i + k] << (8 * k);
            if ((data[i + offSize - 1] & 0x80) != 0) delta |= -1L << (8 * offSize); // sign-extend
            i += offSize;
            lcn += delta;
            runs.Add((lcn, length));
        }
        return runs;
    }

    private void ParseRecord(Span<byte> rec, int frn)
    {
        if (!ApplyFixups(rec)) return;
        ushort flags = BinaryPrimitives.ReadUInt16LittleEndian(rec[0x16..]);
        if ((flags & 1) == 0) return; // not in use
        long baseRef = BinaryPrimitives.ReadInt64LittleEndian(rec[0x20..]) & 0xFFFFFFFFFFFF;
        int owner = baseRef != 0 ? (int)baseRef : frn;
        if (owner < 0 || owner >= _flags.Length) return;
        if (baseRef == 0)
        {
            _flags[owner] |= 1;
            if ((flags & 2) != 0) _flags[owner] |= 2;
        }

        int off = BinaryPrimitives.ReadUInt16LittleEndian(rec[0x14..]);
        while (off + 16 <= rec.Length)
        {
            uint type = BinaryPrimitives.ReadUInt32LittleEndian(rec[off..]);
            if (type == 0xFFFFFFFF) break;
            int len = (int)BinaryPrimitives.ReadUInt32LittleEndian(rec[(off + 4)..]);
            if (len <= 0 || off + len > rec.Length) break;
            bool nonResident = rec[off + 8] != 0;
            int nameLen = rec[off + 9];
            switch (type)
            {
                case 0x10 when !nonResident: // $STANDARD_INFORMATION
                {
                    int v = BinaryPrimitives.ReadUInt16LittleEndian(rec[(off + 0x14)..]) + off;
                    if (v + 0x24 > rec.Length) break;
                    long created = BinaryPrimitives.ReadInt64LittleEndian(rec[v..]);
                    long modified = BinaryPrimitives.ReadInt64LittleEndian(rec[(v + 8)..]);
                    long accessed = BinaryPrimitives.ReadInt64LittleEndian(rec[(v + 24)..]);
                    uint attrs = BinaryPrimitives.ReadUInt32LittleEndian(rec[(v + 32)..]);
                    _created[owner] = created;
                    _write[owner] = modified;
                    _used[owner] = Math.Max(modified, accessed);
                    if ((attrs & Native.FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0) _flags[owner] |= 16;
                    break;
                }
                case 0x30 when !nonResident: // $FILE_NAME
                {
                    int v = BinaryPrimitives.ReadUInt16LittleEndian(rec[(off + 0x14)..]) + off;
                    if (v + 0x42 > rec.Length) break;
                    byte ns = rec[v + 0x41];
                    if (ns == 2) break; // DOS 8.3 alias
                    if ((_flags[owner] & 4) != 0) break; // keep the first name of a hard-linked file
                    int chars = rec[v + 0x40];
                    if (v + 0x42 + chars * 2 > rec.Length) break;
                    long parent = BinaryPrimitives.ReadInt64LittleEndian(rec[v..]) & 0xFFFFFFFFFFFF;
                    _parent[owner] = (int)parent;
                    _name[owner] = new string(MemoryMarshal.Cast<byte, char>(rec.Slice(v + 0x42, chars * 2)));
                    _flags[owner] |= 4;
                    break;
                }
                case 0x80 when nameLen == 0: // unnamed $DATA
                {
                    long size;
                    if (nonResident)
                    {
                        long lowVcn = BinaryPrimitives.ReadInt64LittleEndian(rec[(off + 0x10)..]);
                        if (lowVcn != 0) break; // continuation: size is stored in the first piece
                        size = BinaryPrimitives.ReadInt64LittleEndian(rec[(off + 0x30)..]);
                    }
                    else
                    {
                        size = BinaryPrimitives.ReadUInt32LittleEndian(rec[(off + 0x10)..]);
                    }
                    _size[owner] = size;
                    break;
                }
                case 0xC0: // $REPARSE_POINT
                {
                    if (nonResident) break;
                    int v = BinaryPrimitives.ReadUInt16LittleEndian(rec[(off + 0x14)..]) + off;
                    if (v + 4 > rec.Length) break;
                    uint tag = BinaryPrimitives.ReadUInt32LittleEndian(rec[v..]);
                    if (tag is Native.IO_REPARSE_TAG_MOUNT_POINT or Native.IO_REPARSE_TAG_SYMLINK) _flags[owner] |= 8;
                    break;
                }
            }
            off += len;
        }
    }

    private DirNode BuildTree(string volumeRoot, string requested, CancellationToken ct)
    {
        int n = _flags.Length;
        // children lists as linked arrays: head[parent] → first child, next[child] → sibling
        var headDir = new int[n];
        var headFile = new int[n];
        var next = new int[n];
        Array.Fill(headDir, -1);
        Array.Fill(headFile, -1);
        for (int i = 0; i < n; i++)
        {
            if ((_flags[i] & 5) != 5 || i == RootFrn) continue;
            int p = _parent[i];
            if (p < 0 || p >= n || p == i) continue;
            if ((_flags[i] & 2) != 0)
            {
                next[i] = headDir[p];
                headDir[p] = i;
            }
            else
            {
                next[i] = headFile[p];
                headFile[p] = i;
            }
        }

        var root = new DirNode { Name = volumeRoot };
        Build(root, RootFrn, headDir, headFile, next, ct);
        root.CategorySizes = null;

        // a sub-folder was requested: walk down to it
        string rel = requested.Substring(volumeRoot.Length).Trim('\\');
        if (rel.Length == 0) return root;
        var node = root;
        foreach (var part in rel.Split('\\', StringSplitOptions.RemoveEmptyEntries))
        {
            node = node.Dirs.FirstOrDefault(d => d.Name.Equals(part, StringComparison.OrdinalIgnoreCase));
            if (node == null) throw new DirectoryNotFoundException(requested);
        }
        node.Parent = null;
        node.Name = requested.TrimEnd('\\');
        return node;
    }

    private void Build(DirNode node, int frn, int[] headDir, int[] headFile, int[] next, CancellationToken ct)
    {
        ct.ThrowIfCancellationRequested();
        var cats = new long[FileCategories.All.Length];
        long size = 0, files = 0, dirs = 0;

        var fileList = new List<FileEntry>();
        for (int f = headFile[frn]; f >= 0; f = next[f])
        {
            string name = _name[f];
            if (frn == RootFrn && name.StartsWith('$') && f < 24 && name != "$MFT" && name != "$LogFile") continue; // tiny metadata files
            long s = (_flags[f] & 16) != 0 ? 0 : _size[f];
            byte cat = (byte)FileCategories.Of(name);
            fileList.Add(new FileEntry { Name = name, Size = s, LastWrite = _write[f], LastUsed = _used[f], Created = _created[f], Category = cat });
            cats[cat] += s;
            size += s;
        }
        if (fileList.Count > 0)
        {
            var arr = fileList.ToArray();
            Array.Sort(arr, static (a, b) => b.Size.CompareTo(a.Size));
            node.Files = arr;
            files = arr.Length;
            Interlocked.Add(ref _bytes, size);
        }

        var dirList = new List<DirNode>();
        for (int d = headDir[frn]; d >= 0; d = next[d])
        {
            if ((_flags[d] & 8) != 0) continue; // junctions and symbolic links are not followed
            if (frn == RootFrn && _name[d] == "$Extend") continue;
            var child = new DirNode { Name = _name[d], Parent = node, LastWrite = _write[d] };
            Build(child, d, headDir, headFile, next, ct);
            dirList.Add(child);
            size += child.Size;
            files += child.FileCount;
            dirs += child.DirCount + 1;
            if (child.CategorySizes != null)
            {
                for (int i = 0; i < cats.Length; i++) cats[i] += child.CategorySizes[i];
                child.CategorySizes = null;
            }
        }
        if (dirList.Count > 0)
        {
            dirList.Sort(static (a, b) => b.Size.CompareTo(a.Size));
            node.Dirs = dirList;
        }

        node.Size = size;
        node.FileCount = files;
        node.DirCount = dirs;
        node.CategorySizes = cats;
        int best = (int)FileCategory.Other;
        for (int i = 0; i < cats.Length; i++)
            if (cats[i] > cats[best]) best = i;
        node.Dominant = (byte)best;
    }
}
