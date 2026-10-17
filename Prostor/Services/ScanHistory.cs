using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

public sealed class ScanSnapshot
{
    public string Root { get; set; }
    public DateTime Time { get; set; }
    public long Total { get; set; }
    /// <summary>Relative folder path → size (folders of at least 1 MB, up to 7 levels deep).</summary>
    public Dictionary<string, long> Dirs { get; set; } = new();
}

public sealed class FolderChange
{
    public string RelativePath { get; init; }
    public string FullPath { get; init; }
    public long OldSize { get; init; }
    public long NewSize { get; init; }
    public long Delta => NewSize - OldSize;
    public bool IsNew => OldSize == 0;
    public bool IsGone => NewSize == 0;

    public string DeltaText => (Delta > 0 ? "+" : "−") + Fmt.Size(Math.Abs(Delta));
    public string Detail => IsNew ? "новая папка" : IsGone ? "папки больше нет" : $"{Fmt.Size(OldSize)} → {Fmt.Size(NewSize)}";
    public bool Grew => Delta > 0;
}

public sealed class ScanComparison
{
    public DateTime PreviousTime { get; init; }
    public long PreviousTotal { get; init; }
    public long CurrentTotal { get; init; }
    public List<FolderChange> Changes { get; init; } = new();
    public long Delta => CurrentTotal - PreviousTotal;
}

/// <summary>Remembers the folder sizes of every scan to show what grew since last time.</summary>
public static class ScanHistory
{
    private const int MaxDepth = 7;
    private const long MinDirSize = 1L << 20;
    private const int KeepPerRoot = 6;

    private static string Folder => Path.Combine(AppSettings.DataFolder, "Scans");

    private static string Key(string root)
    {
        var hash = SHA1.HashData(Encoding.UTF8.GetBytes(root.TrimEnd('\\').ToLowerInvariant()));
        return Convert.ToHexString(hash, 0, 8);
    }

    public static ScanSnapshot Capture(DirNode root, string rootPath)
    {
        var snap = new ScanSnapshot { Root = rootPath.TrimEnd('\\'), Time = DateTime.Now, Total = root.Size };
        void Walk(DirNode node, string rel, int depth)
        {
            foreach (var d in node.Dirs)
            {
                if (d.IsVirtual || d.Size < MinDirSize) continue;
                string path = rel.Length == 0 ? d.Name : rel + "\\" + d.Name;
                snap.Dirs[path] = d.Size;
                if (depth < MaxDepth) Walk(d, path, depth + 1);
            }
        }
        Walk(root, "", 1);
        return snap;
    }

    public static void Save(ScanSnapshot snap)
    {
        try
        {
            Directory.CreateDirectory(Folder);
            string key = Key(snap.Root);
            string file = Path.Combine(Folder, $"{key}_{snap.Time:yyyyMMdd_HHmmss}.json.gz");
            using (var fs = File.Create(file))
            using (var gz = new GZipStream(fs, CompressionLevel.Fastest))
                JsonSerializer.Serialize(gz, snap);
            foreach (var old in Directory.GetFiles(Folder, key + "_*.json.gz").OrderByDescending(f => f).Skip(KeepPerRoot))
                File.Delete(old);
        }
        catch { }
    }

    /// <summary>The newest earlier scan of the same folder that is at least <paramref name="minAge"/> old.</summary>
    public static ScanSnapshot LoadPrevious(string root, TimeSpan minAge)
    {
        try
        {
            if (!Directory.Exists(Folder)) return null;
            string key = Key(root.TrimEnd('\\'));
            foreach (var file in Directory.GetFiles(Folder, key + "_*.json.gz").OrderByDescending(f => f))
            {
                using var fs = File.OpenRead(file);
                using var gz = new GZipStream(fs, CompressionMode.Decompress);
                var snap = JsonSerializer.Deserialize<ScanSnapshot>(gz);
                if (snap != null && DateTime.Now - snap.Time >= minAge) return snap;
            }
        }
        catch { }
        return null;
    }

    /// <summary>Folders whose size changed noticeably; a parent is dropped when one child explains most of its change.</summary>
    public static ScanComparison Compare(ScanSnapshot previous, ScanSnapshot current, int max = 40)
    {
        long threshold = Math.Max(50L << 20, Math.Max(previous.Total, current.Total) / 1000);
        var keys = new HashSet<string>(previous.Dirs.Keys, StringComparer.OrdinalIgnoreCase);
        keys.UnionWith(current.Dirs.Keys);
        var deltas = new Dictionary<string, long>(StringComparer.OrdinalIgnoreCase);
        foreach (var k in keys)
        {
            previous.Dirs.TryGetValue(k, out long a);
            current.Dirs.TryGetValue(k, out long b);
            // folders below 1 MB are not stored: a missing entry of a deep folder means "small", not "gone",
            // unless its parent is present in that snapshot
            if (b - a is var d && Math.Abs(d) >= threshold) deltas[k] = d;
        }

        var result = new List<FolderChange>();
        foreach (var (path, delta) in deltas)
        {
            string prefix = path + "\\";
            bool explainedByChild = deltas.Any(c => c.Key.StartsWith(prefix, StringComparison.OrdinalIgnoreCase) &&
                                                    Math.Sign(c.Value) == Math.Sign(delta) && Math.Abs(c.Value) >= Math.Abs(delta) * 0.8);
            if (explainedByChild) continue;
            previous.Dirs.TryGetValue(path, out long oldSize);
            current.Dirs.TryGetValue(path, out long newSize);
            result.Add(new FolderChange
            {
                RelativePath = path,
                FullPath = Path.Combine(current.Root.Length <= 2 ? current.Root + "\\" : current.Root, path),
                OldSize = oldSize,
                NewSize = newSize
            });
        }
        return new ScanComparison
        {
            PreviousTime = previous.Time,
            PreviousTotal = previous.Total,
            CurrentTotal = current.Total,
            Changes = result.OrderByDescending(c => Math.Abs(c.Delta)).Take(max).ToList()
        };
    }
}
