namespace Prostor.Models;

public enum NodeKind : byte
{
    Normal,
    /// <summary>Virtual node for space the scan could not attribute (system / access denied).</summary>
    Unknown
}

public struct FileEntry
{
    public string Name;
    public long Size;
    public long LastWrite;
    /// <summary>Latest of last access and last write (FILETIME): when the file was last used.</summary>
    public long LastUsed;
    /// <summary>Creation time (FILETIME): for downloads this is when the file arrived.</summary>
    public long Created;
    public byte Category;
}

/// <summary>Identifies a file inside the scanned tree (used for selection and context actions).</summary>
public sealed record FileRef(DirNode Parent, string Name);

public sealed class DirNode
{
    public string Name;
    public DirNode Parent;
    public List<DirNode> Dirs = new(0);
    public FileEntry[] Files = Array.Empty<FileEntry>();
    public long Size;
    public long FileCount;
    public long DirCount;
    public long LastWrite;
    public bool AccessDenied;
    public NodeKind Kind;
    /// <summary>Category that takes the most space inside this folder (used to colour collapsed folders).</summary>
    public byte Dominant = (byte)FileCategory.Other;
    /// <summary>Per-category totals, kept only until the parent folder has consumed them during the scan.</summary>
    internal long[] CategorySizes;

    public bool IsVirtual => Kind == NodeKind.Unknown;

    public string FullPath => Parent == null ? Name : Path.Combine(Parent.FullPath, Name);

    public bool IsAncestorOf(DirNode node)
    {
        for (var p = node; p != null; p = p.Parent)
            if (p == this) return true;
        return false;
    }

    /// <summary>Removes a child folder and updates the totals of all ancestors.</summary>
    public void RemoveDir(DirNode child)
    {
        if (!Dirs.Remove(child)) return;
        Propagate(-child.Size, -child.FileCount, -(child.DirCount + 1));
    }

    public bool RemoveFile(string name)
    {
        int idx = Array.FindIndex(Files, f => string.Equals(f.Name, name, StringComparison.OrdinalIgnoreCase));
        if (idx < 0) return false;
        long size = Files[idx].Size;
        var list = new List<FileEntry>(Files);
        list.RemoveAt(idx);
        Files = list.ToArray();
        Propagate(-size, -1, 0);
        return true;
    }

    private void Propagate(long sizeDelta, long filesDelta, long dirsDelta)
    {
        for (var p = this; p != null; p = p.Parent)
        {
            p.Size += sizeDelta;
            p.FileCount += filesDelta;
            p.DirCount += dirsDelta;
            p.Parent?.Dirs.Sort(static (a, b) => b.Size.CompareTo(a.Size));
        }
    }
}
