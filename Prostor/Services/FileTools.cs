using System.IO.Compression;
using Prostor.Core;

namespace Prostor.Services;

/// <summary>NTFS (WOF, the same as "compact /exe:lzx" and CompactGUI) compression of folders.</summary>
public static class Compression
{
    /// <summary>File types that are already compressed — WOF would gain nothing.</summary>
    private static readonly HashSet<string> Incompressible = new(StringComparer.OrdinalIgnoreCase)
    {
        ".zip", ".7z", ".rar", ".gz", ".xz", ".bz2", ".zst", ".cab", ".jpg", ".jpeg", ".png", ".gif", ".webp", ".avif", ".heic",
        ".mp4", ".mkv", ".avi", ".mov", ".webm", ".mp3", ".ogg", ".opus", ".flac", ".aac", ".m4a", ".wma", ".bik", ".bk2",
        ".usm", ".pdf", ".docx", ".xlsx", ".pptx", ".jar", ".apk", ".msi", ".vpk", ".pak", ".utoc", ".ucas", ".iso", ".wim", ".esd"
    };

    public static bool IsSupported(string folder)
    {
        try
        {
            string root = Path.GetPathRoot(Path.GetFullPath(folder));
            return root != null && string.Equals(new DriveInfo(root).DriveFormat, "NTFS", StringComparison.OrdinalIgnoreCase);
        }
        catch
        {
            return false;
        }
    }

    /// <summary>Space the folder really takes on disk (compressed files count with their compressed size).</summary>
    public static long PhysicalSize(string folder, CancellationToken ct = default)
    {
        long total = 0;
        foreach (var file in EnumerateFiles(folder, ct))
        {
            uint low = Native.GetCompressedFileSize(file, out uint high);
            if (low == 0xFFFFFFFF && System.Runtime.InteropServices.Marshal.GetLastWin32Error() != 0) continue;
            total += ((long)high << 32) | low;
        }
        return total;
    }

    /// <summary>Rough share of space that compression would save, measured on a sample of the files.</summary>
    public static double EstimateSaving(string folder, CancellationToken ct = default)
    {
        var files = EnumerateFiles(folder, ct)
            .Select(f => { try { return new FileInfo(f); } catch { return null; } })
            .Where(f => f is { Length: > 4096 })
            .ToList();
        if (files.Count == 0) return 0;
        long totalBytes = files.Sum(f => f.Length);
        // weight the sample by size: the biggest files decide the result
        var sample = files.OrderByDescending(f => f.Length).Take(60)
            .Concat(files.Where((_, i) => i % Math.Max(1, files.Count / 60) == 0).Take(60))
            .DistinctBy(f => f.FullName)
            .ToList();
        double sampledBytes = 0, savedBytes = 0;
        var buffer = new byte[256 * 1024];
        foreach (var f in sample)
        {
            ct.ThrowIfCancellationRequested();
            if (Incompressible.Contains(f.Extension))
            {
                sampledBytes += f.Length;
                continue;
            }
            try
            {
                using var fs = new FileStream(f.FullName, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
                int read = fs.Read(buffer, 0, buffer.Length);
                if (read <= 0) continue;
                using var ms = new MemoryStream();
                using (var z = new DeflateStream(ms, CompressionLevel.Fastest, true)) z.Write(buffer, 0, read);
                double ratio = Math.Min(1, (double)ms.Length / read);
                sampledBytes += f.Length;
                savedBytes += f.Length * (1 - ratio) * 0.95;
            }
            catch { }
        }
        if (sampledBytes <= 0) return 0;
        double saving = savedBytes / sampledBytes;
        return Math.Clamp(saving * Math.Min(1, sampledBytes / Math.Max(1, totalBytes) * 4 + 0.75), 0, 0.85);
    }

    public static Task CompressAsync(string folder, IProgress<(int done, int total)> progress, CancellationToken ct) =>
        RunCompact(folder, $"/c /s:{Cmd.Quote(folder.TrimEnd('\\'))} /a /i /q /exe:lzx", progress, ct);

    public static Task DecompressAsync(string folder, IProgress<(int done, int total)> progress, CancellationToken ct) =>
        RunCompact(folder, $"/u /s:{Cmd.Quote(folder.TrimEnd('\\'))} /a /i /q /exe", progress, ct);

    private static async Task RunCompact(string folder, string args, IProgress<(int, int)> progress, CancellationToken ct)
    {
        int total = await Task.Run(() => EnumerateFiles(folder, ct).Count(), ct);
        int done = 0;
        await Cmd.RunAsync(Cmd.SystemTool("compact.exe"), args, null, 6 * 3600_000, line =>
        {
            // every processed file prints one line with its name; headers end with ':' or are empty
            if (string.IsNullOrWhiteSpace(line) || line.TrimEnd().EndsWith(':')) return;
            done = Math.Min(total, done + 1);
            progress?.Report((done, total));
        }, ct);
        progress?.Report((total, total));
    }

    internal static IEnumerable<string> EnumerateFiles(string folder, CancellationToken ct)
    {
        var options = new EnumerationOptions
        {
            RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.ReparsePoint,
            ReturnSpecialDirectories = false
        };
        IEnumerator<string> e;
        try { e = Directory.EnumerateFiles(folder, "*", options).GetEnumerator(); }
        catch { yield break; }
        using (e)
        {
            while (true)
            {
                ct.ThrowIfCancellationRequested();
                bool next;
                try { next = e.MoveNext(); }
                catch { yield break; }
                if (!next) yield break;
                yield return e.Current;
            }
        }
    }
}

/// <summary>Moves a folder to another drive and leaves a junction in its place, so programs keep finding it.</summary>
public static class FolderMover
{
    public static string CheckCanMove(string folder)
    {
        try
        {
            string full = Path.GetFullPath(folder).TrimEnd('\\');
            if (full.Length <= 3) return "Нельзя перенести корень диска.";
            var info = new DirectoryInfo(full);
            if (!info.Exists) return "Папка не найдена.";
            if (info.Attributes.HasFlag(FileAttributes.ReparsePoint)) return "Это уже ссылка на другую папку.";
            string win = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
            if (full.StartsWith(win, StringComparison.OrdinalIgnoreCase)) return "Системные папки Windows переносить нельзя.";
            string[] roots =
            {
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86),
                Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "AppData"),
                Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), @"AppData\LocalLow"),
                @"C:\Users",
            };
            if (roots.Any(r => !string.IsNullOrEmpty(r) && r.TrimEnd('\\').Equals(full, StringComparison.OrdinalIgnoreCase)))
                return "Эту папку Windows ожидает на прежнем месте — перенесите вложенную папку.";
            if (full.Contains(@"\$Recycle.Bin", StringComparison.OrdinalIgnoreCase) ||
                full.Contains("System Volume Information", StringComparison.OrdinalIgnoreCase))
                return "Служебные папки переносить нельзя.";
            return null;
        }
        catch (Exception ex)
        {
            return ex.Message;
        }
    }

    /// <summary>Other fixed NTFS drives with enough free space.</summary>
    public static List<DriveInfo> TargetDrives(string folder, long size)
    {
        string root = Path.GetPathRoot(Path.GetFullPath(folder));
        return DriveInfo.GetDrives().Where(d =>
        {
            try
            {
                return d.IsReady && d.DriveType == DriveType.Fixed && !d.Name.Equals(root, StringComparison.OrdinalIgnoreCase) &&
                       d.DriveFormat == "NTFS" && d.AvailableFreeSpace > size + (512L << 20);
            }
            catch
            {
                return false;
            }
        }).ToList();
    }

    /// <returns>The new location.</returns>
    public static async Task<string> MoveAsync(string source, string targetParent, IProgress<double> progress, CancellationToken ct)
    {
        source = Path.GetFullPath(source).TrimEnd('\\');
        string reason = CheckCanMove(source);
        if (reason != null) throw new InvalidOperationException(reason);
        string name = Path.GetFileName(source);
        string target = Path.Combine(targetParent, name);
        for (int i = 2; Directory.Exists(target) || File.Exists(target); i++) target = Path.Combine(targetParent, $"{name} ({i})");

        // a folder that some program keeps open cannot be renamed — find out before copying gigabytes
        string probe = source + ".prostor-check";
        try
        {
            Directory.Move(source, probe);
            Directory.Move(probe, source);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            if (Directory.Exists(probe) && !Directory.Exists(source)) Directory.Move(probe, source);
            throw new InvalidOperationException("Папка сейчас используется какой-то программой. Закройте её и повторите.");
        }

        var (bytes, files) = await Task.Run(() => Measure(source, ct), ct);
        long copied = 0;
        try
        {
            await Task.Run(() => CopyTree(source, target, ref copied, bytes, progress, ct), ct);
            var (b2, f2) = await Task.Run(() => Measure(target, ct), ct);
            if (b2 != bytes || f2 != files) throw new IOException("Копия не совпадает с оригиналом — перенос отменён.");
        }
        catch
        {
            TryDelete(target);
            throw;
        }

        string old = source + ".prostor-old";
        Directory.Move(source, old);
        var link = await Cmd.RunAsync(Environment.GetEnvironmentVariable("ComSpec") ?? Cmd.SystemTool("cmd.exe"),
            $"/c mklink /J {Cmd.Quote(source)} {Cmd.Quote(target)}");
        if (!link.Ok || !Directory.Exists(source))
        {
            Directory.Move(old, source);
            TryDelete(target);
            throw new IOException("Не удалось создать ссылку на новое место: " + link.Output.Trim());
        }
        await Task.Run(() => TryDelete(old));
        return target;
    }

    public static async Task MoveBackAsync(string original, string current, IProgress<double> progress, CancellationToken ct)
    {
        var info = new DirectoryInfo(original);
        if (info.Exists && !info.Attributes.HasFlag(FileAttributes.ReparsePoint))
            throw new InvalidOperationException("На старом месте уже лежит обычная папка.");
        if (!Directory.Exists(current)) throw new InvalidOperationException("Перенесённая папка не найдена: " + current);
        var (bytes, files) = await Task.Run(() => Measure(current, ct), ct);
        if (info.Exists) Directory.Delete(original); // removes only the junction
        long copied = 0;
        try
        {
            await Task.Run(() => CopyTree(current, original, ref copied, bytes, progress, ct), ct);
            var (b2, f2) = await Task.Run(() => Measure(original, ct), ct);
            if (b2 != bytes || f2 != files) throw new IOException("Копия не совпадает с оригиналом.");
        }
        catch
        {
            TryDelete(original);
            await Cmd.RunAsync(Cmd.SystemTool("cmd.exe"), $"/c mklink /J {Cmd.Quote(original)} {Cmd.Quote(current)}");
            throw;
        }
        await Task.Run(() => TryDelete(current));
    }

    private static (long bytes, long files) Measure(string folder, CancellationToken ct)
    {
        long bytes = 0, files = 0;
        foreach (var f in Compression.EnumerateFiles(folder, ct))
        {
            try
            {
                bytes += new FileInfo(f).Length;
                files++;
            }
            catch { }
        }
        return (bytes, files);
    }

    private static void CopyTree(string src, string dst, ref long copied, long total, IProgress<double> progress, CancellationToken ct)
    {
        Directory.CreateDirectory(dst);
        var srcInfo = new DirectoryInfo(src);
        var buffer = new byte[1 << 20];
        foreach (var entry in srcInfo.EnumerateFileSystemInfos())
        {
            ct.ThrowIfCancellationRequested();
            if (entry.Attributes.HasFlag(FileAttributes.ReparsePoint)) continue;
            string target = Path.Combine(dst, entry.Name);
            if (entry is DirectoryInfo d)
            {
                CopyTree(d.FullName, target, ref copied, total, progress, ct);
                try { new DirectoryInfo(target).Attributes = d.Attributes; } catch { }
                continue;
            }
            var f = (FileInfo)entry;
            using (var input = new FileStream(f.FullName, FileMode.Open, FileAccess.Read, FileShare.Read, 1 << 16, FileOptions.SequentialScan))
            using (var output = new FileStream(target, FileMode.CreateNew, FileAccess.Write, FileShare.None, 1 << 16))
            {
                int n;
                while ((n = input.Read(buffer, 0, buffer.Length)) > 0)
                {
                    ct.ThrowIfCancellationRequested();
                    output.Write(buffer, 0, n);
                    copied += n;
                    if (total > 0) progress?.Report(Math.Min(1, (double)copied / total));
                }
            }
            var t = new FileInfo(target);
            t.CreationTimeUtc = f.CreationTimeUtc;
            t.LastWriteTimeUtc = f.LastWriteTimeUtc;
            t.Attributes = f.Attributes;
        }
    }

    private static void TryDelete(string folder)
    {
        try
        {
            if (!Directory.Exists(folder)) return;
            foreach (var f in Directory.EnumerateFiles(folder, "*", new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true, AttributesToSkip = FileAttributes.ReparsePoint }))
            {
                try { File.SetAttributes(f, FileAttributes.Normal); } catch { }
            }
            Directory.Delete(folder, true);
        }
        catch { }
    }
}
