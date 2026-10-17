using System.Globalization;
using System.Management;
using System.Text.RegularExpressions;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

/// <summary>System-level cleanup through Windows' own tools: DISM, powercfg, shadow copies, pnputil.</summary>
public static class DeepCleanup
{
    private static string WinDir => Environment.GetFolderPath(Environment.SpecialFolder.Windows);
    private static string SystemRoot => Path.GetPathRoot(WinDir) ?? "C:\\";

    public static IEnumerable<CleanupItem> Build(string group)
    {
        bool admin = ShellHelper.IsAdmin;

        yield return new CleanupItem
        {
            Id = "winsxs", Group = group, Glyph = "\uE896", RequiresAdmin = true, IsSlow = true,
            Title = "Устаревшие компоненты Windows (WinSxS)",
            Description = "Старые версии системных файлов, оставшиеся после обновлений. Удаляет сама Windows (DISM). Займёт несколько минут.",
            Warning = "После очистки установленные обновления нельзя будет удалить.",
            CustomMeasure = _ => (CleanupItem.UnknownSize, 0),
            CustomClean = ct => FreedOnSystemDrive(() =>
                Cmd.Run(Cmd.SystemTool("Dism.exe"), "/Online /Cleanup-Image /StartComponentCleanup", timeoutMs: 2 * 3600_000, ct: ct))
        };

        string hiber = Path.Combine(SystemRoot, "hiberfil.sys");
        long hiberSize = FileSize(hiber);
        if (hiberSize > 0)
            yield return new CleanupItem
            {
                Id = "hibernation", Group = group, Glyph = "\uE708", RequiresAdmin = true,
                Title = "Файл гибернации",
                Description = "hiberfil.sys занимает столько, сколько оперативной памяти. Нужен только для гибернации и «быстрого запуска».",
                Warning = "Гибернация и быстрый запуск Windows будут выключены. Вернуть можно в «Журнале».",
                CustomMeasure = _ => (FileSize(hiber), 1),
                CustomClean = _ =>
                {
                    long before = FileSize(hiber);
                    var r = Cmd.Run(Cmd.SystemTool("powercfg.exe"), "/hibernate off");
                    long after = FileSize(hiber);
                    return new CleanResult { FreedBytes = r.Ok ? Math.Max(0, before - after) : 0, DeletedFiles = after == 0 ? 1 : 0, FailedFiles = after == 0 ? 0 : 1 };
                }
            };

        yield return new CleanupItem
        {
            Id = "restorepoints", Group = group, Glyph = "\uE777", RequiresAdmin = true, IsEstimate = true, CountForms = new[] { "старая точка", "старые точки", "старых точек" },
            Title = "Старые точки восстановления",
            Description = "Все точки восстановления системы, кроме самой новой.",
            Warning = "Откатить систему можно будет только к последней точке восстановления.",
            CustomMeasure = _ => admin ? MeasureShadows() : (0, 0),
            CustomClean = _ => DeleteOldShadows()
        };

        yield return new CleanupItem
        {
            Id = "olddrivers", Group = group, Glyph = "\uE964", RequiresAdmin = true, CountForms = new[] { "пакет", "пакета", "пакетов" },
            Title = "Старые версии драйверов",
            Description = "Предыдущие версии драйверов, которые Windows хранит после обновления. Драйверы, которые сейчас используются, не удаляются.",
            Warning = "Откатить драйвер к прошлой версии будет нельзя.",
            CustomMeasure = _ => admin ? MeasureOldDrivers() : (0, 0),
            CustomClean = _ => DeleteOldDrivers()
        };
    }

    private static long FileSize(string path)
    {
        try
        {
            var fi = new FileInfo(path);
            return fi.Exists ? fi.Length : 0;
        }
        catch
        {
            return 0;
        }
    }

    private static CleanResult FreedOnSystemDrive(Func<CmdResult> action)
    {
        var drive = new DriveInfo(SystemRoot);
        long before = drive.AvailableFreeSpace;
        var r = action();
        drive = new DriveInfo(SystemRoot);
        long freed = Math.Max(0, drive.AvailableFreeSpace - before);
        return new CleanResult { FreedBytes = freed, DeletedFiles = r.Ok ? 1 : 0, FailedFiles = r.Ok ? 0 : 1 };
    }

    // ------------------------------------------------------------------ shadow copies (restore points)

    private static string SystemVolumeId()
    {
        using var s = new ManagementObjectSearcher($"SELECT DeviceID FROM Win32_Volume WHERE DriveLetter = '{SystemRoot.TrimEnd('\\')}'");
        foreach (ManagementObject v in s.Get()) return v["DeviceID"] as string;
        return null;
    }

    private static List<ManagementObject> SystemShadows()
    {
        string volume = SystemVolumeId();
        var list = new List<ManagementObject>();
        if (volume == null) return list;
        using var s = new ManagementObjectSearcher("SELECT ID, InstallDate, VolumeName FROM Win32_ShadowCopy");
        foreach (ManagementObject o in s.Get())
            if (string.Equals(o["VolumeName"] as string, volume, StringComparison.OrdinalIgnoreCase)) list.Add(o);
        return list.OrderBy(o => o["InstallDate"] as string).ToList();
    }

    private static long ShadowStorageUsed()
    {
        string guid = Regex.Match(SystemVolumeId() ?? "", @"\{[0-9a-fA-F-]+\}").Value;
        if (guid.Length == 0) return 0;
        using var s = new ManagementObjectSearcher("SELECT Volume, UsedSpace FROM Win32_ShadowStorage");
        long used = 0;
        foreach (ManagementObject o in s.Get())
        {
            // Volume is a reference like Win32_Volume.DeviceID="\\\\?\\Volume{…}\\"
            if ((o["Volume"] as string ?? "").Contains(guid, StringComparison.OrdinalIgnoreCase))
                used += Convert.ToInt64(o["UsedSpace"]);
        }
        return used;
    }

    private static (long size, long count) MeasureShadows()
    {
        try
        {
            var shadows = SystemShadows();
            if (shadows.Count <= 1) return (0, 0);
            long used = ShadowStorageUsed();
            return (used * (shadows.Count - 1) / shadows.Count, shadows.Count - 1);
        }
        catch
        {
            return (0, 0);
        }
    }

    private static CleanResult DeleteOldShadows()
    {
        var r = new CleanResult();
        long before = ShadowStorageUsed();
        var shadows = SystemShadows();
        foreach (var old in shadows.Take(shadows.Count - 1))
        {
            try
            {
                old.Delete();
                r.DeletedFiles++;
            }
            catch
            {
                r.FailedFiles++;
            }
        }
        r.FreedBytes = Math.Max(0, before - ShadowStorageUsed());
        return r;
    }

    // ------------------------------------------------------------------ driver store

    public sealed record DriverPackage(string Published, string Original, DateTime Date, Version Version);

    private static readonly Regex VersionLine = new(@"(\d{2})/(\d{2})/(\d{4})\s+(\d+(\.\d+){1,3})", RegexOptions.Compiled);

    /// <summary>Parses "pnputil /enum-drivers" (labels are localized, so values are recognized by their shape).</summary>
    public static List<DriverPackage> ParsePnputil(string output)
    {
        var list = new List<DriverPackage>();
        foreach (var block in Regex.Split(output.Replace("\r", ""), @"\n\s*\n"))
        {
            var values = block.Split('\n')
                .Select(l => l.IndexOf(':') is var i and > 0 ? l[(i + 1)..].Trim() : null)
                .Where(v => !string.IsNullOrEmpty(v))
                .ToList();
            string published = values.FirstOrDefault(v => Regex.IsMatch(v, @"^oem\d+\.inf$", RegexOptions.IgnoreCase));
            if (published == null) continue;
            string original = values.FirstOrDefault(v => v.EndsWith(".inf", StringComparison.OrdinalIgnoreCase) && !v.Equals(published, StringComparison.OrdinalIgnoreCase));
            var m = values.Select(v => VersionLine.Match(v)).FirstOrDefault(x => x.Success);
            if (original == null || m == null) continue;
            DateTime.TryParseExact($"{m.Groups[1].Value}/{m.Groups[2].Value}/{m.Groups[3].Value}", "MM/dd/yyyy", CultureInfo.InvariantCulture,
                DateTimeStyles.None, out var date);
            Version.TryParse(m.Groups[4].Value, out var version);
            list.Add(new DriverPackage(published, original, date, version ?? new Version(0, 0)));
        }
        return list;
    }

    /// <summary>All but the newest package of every driver.</summary>
    public static List<DriverPackage> OutdatedPackages(IEnumerable<DriverPackage> packages) =>
        packages.GroupBy(p => p.Original, StringComparer.OrdinalIgnoreCase)
            .Where(g => g.Count() > 1)
            .SelectMany(g => g.OrderByDescending(p => p.Version).ThenByDescending(p => p.Date).Skip(1))
            .ToList();

    private static List<DriverPackage> _outdated;
    private static Dictionary<string, string> _folders;

    private static (long size, long count) MeasureOldDrivers()
    {
        try
        {
            var r = Cmd.Run(Cmd.SystemTool("pnputil.exe"), "/enum-drivers", timeoutMs: 60_000);
            _outdated = OutdatedPackages(ParsePnputil(r.Output));
            _folders = MapRepositoryFolders(_outdated);
            long size = _folders.Values.Sum(f => FastDir.GetSize(f));
            return (size, _outdated.Count);
        }
        catch
        {
            return (0, 0);
        }
    }

    /// <summary>oemNN.inf → its folder in DriverStore\FileRepository (same .inf content).</summary>
    private static Dictionary<string, string> MapRepositoryFolders(List<DriverPackage> packages)
    {
        var map = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        string repo = Path.Combine(Environment.SystemDirectory, @"DriverStore\FileRepository");
        foreach (var p in packages)
        {
            try
            {
                var oem = File.ReadAllBytes(Path.Combine(WinDir, "INF", p.Published));
                string prefix = Path.GetFileNameWithoutExtension(p.Original) + ".inf_";
                foreach (var dir in Directory.EnumerateDirectories(repo, prefix + "*"))
                {
                    string inf = Path.Combine(dir, p.Original);
                    if (File.Exists(inf) && File.ReadAllBytes(inf).AsSpan().SequenceEqual(oem))
                    {
                        map[p.Published] = dir;
                        break;
                    }
                }
            }
            catch { }
        }
        return map;
    }

    private static CleanResult DeleteOldDrivers()
    {
        var r = new CleanResult();
        if (_outdated == null) MeasureOldDrivers();
        foreach (var p in _outdated ?? new())
        {
            _folders.TryGetValue(p.Published, out var folder);
            long size = folder != null ? FastDir.GetSize(folder) : 0;
            // without /force pnputil refuses to remove a package that a device uses
            var res = Cmd.Run(Cmd.SystemTool("pnputil.exe"), $"/delete-driver {p.Published}", timeoutMs: 120_000);
            if (res.Ok)
            {
                r.DeletedFiles++;
                r.FreedBytes += size;
            }
            else r.FailedFiles++;
        }
        _outdated = null;
        return r;
    }
}
