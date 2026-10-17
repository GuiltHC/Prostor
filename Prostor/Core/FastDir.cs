using System.Runtime.InteropServices;

namespace Prostor.Core;

public readonly struct FindEntry
{
    public readonly string Name;
    public readonly uint Attributes;
    public readonly long Size;
    public readonly long LastWrite;
    public readonly uint ReparseTag;

    public FindEntry(string name, uint attributes, long size, long lastWrite, uint reparseTag)
    {
        Name = name;
        Attributes = attributes;
        Size = size;
        LastWrite = lastWrite;
        ReparseTag = reparseTag;
    }

    public bool IsDirectory => (Attributes & Native.FILE_ATTRIBUTE_DIRECTORY) != 0;

    /// <summary>Junctions and symbolic links: never followed to avoid double counting and loops.</summary>
    public bool IsLink => (Attributes & Native.FILE_ATTRIBUTE_REPARSE_POINT) != 0 &&
                          (ReparseTag == Native.IO_REPARSE_TAG_MOUNT_POINT || ReparseTag == Native.IO_REPARSE_TAG_SYMLINK);

    /// <summary>OneDrive "online-only" files occupy no local space.</summary>
    public bool IsCloudOnly => (Attributes & Native.FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0;

    public long DiskSize => IsCloudOnly ? 0 : Size;

    public DateTime LastWriteTime => LastWrite > 0 ? DateTime.FromFileTime(LastWrite) : DateTime.MinValue;
}

/// <summary>Fast directory enumeration on top of FindFirstFileExW (large fetch, basic info).</summary>
public static class FastDir
{
    public const int FindExInfoBasic = 1;
    public const int FIND_FIRST_EX_LARGE_FETCH = 2;

    public static string ToSearchPattern(string dir)
    {
        string p = dir;
        if (!p.StartsWith(@"\\?\", StringComparison.Ordinal))
        {
            p = p.StartsWith(@"\\", StringComparison.Ordinal) ? @"\\?\UNC\" + p.Substring(2) : @"\\?\" + p;
        }
        return p.TrimEnd('\\') + @"\*";
    }

    public static string Combine(string dir, string name) =>
        dir.EndsWith('\\') ? dir + name : dir + "\\" + name;

    /// <summary>Lists a directory. Returns a Win32 error code (0 on success).</summary>
    public static unsafe int List(string dir, List<FindEntry> output)
    {
        IntPtr h = Native.FindFirstFileEx(ToSearchPattern(dir), FindExInfoBasic, out var d, 0, IntPtr.Zero, FIND_FIRST_EX_LARGE_FETCH);
        if (h == Native.INVALID_HANDLE_VALUE)
        {
            int err = Marshal.GetLastWin32Error();
            return err == 2 || err == 18 ? 0 : err;
        }
        try
        {
            do
            {
                char* n = d.cFileName;
                if (n[0] == '.' && (n[1] == '\0' || (n[1] == '.' && n[2] == '\0'))) continue;
                long size = ((long)d.nFileSizeHigh << 32) | d.nFileSizeLow;
                long ft = ((long)d.ftLastWriteTimeHigh << 32) | d.ftLastWriteTimeLow;
                output.Add(new FindEntry(new string(n), d.dwFileAttributes, size, ft, d.dwReserved0));
            } while (Native.FindNextFile(h, out d));
        }
        finally
        {
            Native.FindClose(h);
        }
        return 0;
    }

    /// <summary>Total size of a folder (links are not followed).</summary>
    public static long GetSize(string dir, CancellationToken ct = default) => GetSize(dir, ct, out _);

    /// <param name="exclude">Sub-folders to skip (e.g. games that are listed as separate programs inside the Steam folder).</param>
    public static long GetSize(string dir, CancellationToken ct, out long files, ISet<string> exclude = null)
    {
        long total = 0, count = 0;
        var stack = new Stack<string>();
        stack.Push(dir);
        var buffer = new List<FindEntry>(256);
        while (stack.Count > 0)
        {
            if (ct.IsCancellationRequested) break;
            string cur = stack.Pop();
            buffer.Clear();
            List(cur, buffer);
            foreach (var e in buffer)
            {
                if (e.IsDirectory)
                {
                    if (e.IsLink) continue;
                    string sub = Combine(cur, e.Name);
                    if (exclude == null || !exclude.Contains(sub)) stack.Push(sub);
                }
                else
                {
                    total += e.DiskSize;
                    count++;
                }
            }
        }
        files = count;
        return total;
    }
}
