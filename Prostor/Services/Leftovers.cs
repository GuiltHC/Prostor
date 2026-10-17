using System.Diagnostics;
using System.Text.RegularExpressions;
using Microsoft.Win32;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

public enum LeftoverKind { Folder, Shortcut, RegistryKey, StartupEntry, Task, Service, FirewallRule }

public sealed class Leftover
{
    public LeftoverKind Kind { get; init; }
    /// <summary>Folder/file path, "HKEY_…\key", task path, service name or program path of firewall rules.</summary>
    public string Target { get; init; }
    /// <summary>Run value name for <see cref="LeftoverKind.StartupEntry"/>.</summary>
    public string ValueName { get; init; }
    public string Title { get; init; }
    public long Size { get; init; }
    public bool NeedsAdmin { get; init; }

    public string KindText => Kind switch
    {
        LeftoverKind.Folder => "Папка",
        LeftoverKind.Shortcut => "Ярлык",
        LeftoverKind.RegistryKey => "Реестр",
        LeftoverKind.StartupEntry => "Автозагрузка",
        LeftoverKind.Task => "Задача планировщика",
        LeftoverKind.Service => "Служба",
        LeftoverKind.FirewallRule => "Брандмауэр",
        _ => ""
    };

    public SelectableItem ToItem() => new()
    {
        Title = Title,
        Subtitle = $"{KindText} · {Target}",
        Detail = Kind is LeftoverKind.Folder ? (Size > 0 ? Fmt.Size(Size) : "пустая") : KindText,
        Size = Size,
        Tag = this,
        IsChecked = true
    };
}

public sealed class LeftoverRemoval
{
    public int Removed;
    public int Failed;
    public long Bytes;
    public List<string> Recycled { get; } = new();
    public List<string> RegBackups { get; } = new();
    public List<string> TaskBackups { get; } = new();
}

/// <summary>Finds what a program leaves behind: folders, shortcuts, registry keys, autostart, tasks, services, firewall rules.</summary>
public static class LeftoverFinder
{
    private static readonly Regex VersionRx = new(@"\s*(v(er(sion)?)?\.?\s*)?\d+(\.\d+)+.*$", RegexOptions.IgnoreCase);
    private static readonly Regex ParensRx = new(@"\s*[\(\[].*?[\)\]]");
    private static readonly Regex ArchRx = new(@"\b(x64|x86|64-bit|32-bit|64 bit|32 bit|amd64|win64|win32)\b", RegexOptions.IgnoreCase);
    private static readonly Regex CompanyRx = new(@"[,\.]?\s*\b(inc|llc|ltd|limited|corp|corporation|co|gmbh|s\.?r\.?o|ag|sa|bv|oy|ab|ооо)\b\.?", RegexOptions.IgnoreCase);

    /// <summary>Vendor / system names shared by many programs — never offered for removal.</summary>
    private static readonly HashSet<string> SharedNames = new(StringComparer.OrdinalIgnoreCase)
    {
        "Microsoft", "Google", "Packages", "Temp", "Programs", "Common Files", "Windows", "Intel", "AMD", "NVIDIA",
        "NVIDIA Corporation", "Mozilla", "Adobe", "Apple", "Oracle", "Package Cache", "Installer", "Downloaded Installations",
        "Microsoft Office", "Windows Kits", "dotnet", "Steam", "Epic Games", "JetBrains", "Yandex", "Valve", "Realtek",
        "Classes", "Policies", "Wow6432Node", "WOW6432Node", "Clients", "RegisteredApplications", "ODBC", "Khronos", "Python",
        "Wow6432", "Intel Corporation", "Advanced Micro Devices, Inc.", "Logitech", "Razer", "Corsair", "Ubisoft", "Electronic Arts",
        "GOG.com", "Blizzard Entertainment", "Battle.net", "Origin", "EA Games", "Unity", "Unreal Engine", "Epic Games Launcher",
    };

    public static List<Leftover> Find(InstalledApp app, IReadOnlyCollection<InstalledApp> remaining, bool includeInstallFolder = true)
    {
        var result = new List<Leftover>();
        var names = CleanNames(app.Name);
        string publisher = CleanPublisher(app.Publisher);
        string location = AppInventory.IsUsableLocationPath(app.InstallLocation) ? app.InstallLocation.TrimEnd('\\') : null;
        var others = remaining.Where(a => a != app).ToList();
        var otherLocations = others.Where(a => !string.IsNullOrEmpty(a.InstallLocation))
            .Select(a => a.InstallLocation.TrimEnd('\\')).ToList();
        var otherNames = new HashSet<string>(others.SelectMany(a => CleanNames(a.Name)), StringComparer.OrdinalIgnoreCase);

        bool Mine(string path) => location != null && IsSameOrParent(location, path);

        try { FindFolders(app, names, publisher, includeInstallFolder ? location : null, otherLocations, result); } catch { }
        try { FindShortcuts(names, location, result); } catch { }
        try { FindRegistry(app, names, publisher, otherNames, result); } catch { }
        if (location != null)
        {
            try { FindStartup(Mine, result); } catch { }
            try { FindTasks(Mine, result); } catch { }
            try { FindServices(Mine, result); } catch { }
            try { FindFirewall(Mine, location, result); } catch { }
        }
        var settings = AppSettings.Current;
        return result.Where(r => !(r.Kind is LeftoverKind.Folder or LeftoverKind.Shortcut && settings.IsExcluded(r.Target))).ToList();
    }

    // ------------------------------------------------------------------ folders

    private static void FindFolders(InstalledApp app, List<string> names, string publisher, string location,
        List<string> otherLocations, List<Leftover> result)
    {
        var candidates = new List<string>();
        if (location != null) candidates.Add(location);
        string[] roots =
        {
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
            Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86),
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), @"AppData\LocalLow"),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData),
        };
        foreach (var root in roots.Where(r => !string.IsNullOrEmpty(r)).Distinct(StringComparer.OrdinalIgnoreCase))
        {
            foreach (var n in names)
            {
                candidates.Add(Path.Combine(root, n));
                if (!string.IsNullOrEmpty(publisher)) candidates.Add(Path.Combine(root, publisher, n));
                if (!string.IsNullOrWhiteSpace(app.Publisher)) candidates.Add(Path.Combine(root, app.Publisher.Trim(), n));
            }
        }

        foreach (var c in candidates.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            try
            {
                if (!AppInventory.IsUsableLocation(c)) continue;
                string full = Path.GetFullPath(c).TrimEnd('\\');
                if (SharedNames.Contains(Path.GetFileName(full))) continue;
                // never touch a folder that still belongs to (or contains) another installed program
                if (otherLocations.Any(o => IsSameOrParent(full, o) || IsSameOrParent(o, full))) continue;
                if (result.Any(r => r.Kind == LeftoverKind.Folder && IsSameOrParent(r.Target, full))) continue;
                long size = FastDir.GetSize(full, CancellationToken.None, out _);
                result.Add(new Leftover { Kind = LeftoverKind.Folder, Target = full, Title = Path.GetFileName(full), Size = size });
            }
            catch { }
        }
    }

    // ------------------------------------------------------------------ shortcuts

    private static void FindShortcuts(List<string> names, string location, List<Leftover> result)
    {
        string[] roots =
        {
            Environment.GetFolderPath(Environment.SpecialFolder.Programs),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms),
            Environment.GetFolderPath(Environment.SpecialFolder.Desktop),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory),
        };
        var type = Type.GetTypeFromProgID("WScript.Shell");
        if (type == null) return;
        dynamic shell = Activator.CreateInstance(type);
        var normNames = names.Select(Normalize).Where(n => n.Length >= 3).ToList();
        foreach (var root in roots.Where(r => !string.IsNullOrEmpty(r) && Directory.Exists(r)))
        {
            foreach (var lnk in Directory.EnumerateFiles(root, "*.lnk", new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = true }))
            {
                string target;
                try { target = (string)shell.CreateShortcut(lnk).TargetPath; }
                catch { continue; }
                bool byTarget = location != null && !string.IsNullOrEmpty(target) && IsSameOrParent(location, target);
                bool broken = !string.IsNullOrEmpty(target) && !File.Exists(target) && !Directory.Exists(target);
                bool byName = broken && normNames.Any(n => Normalize(Path.GetFileNameWithoutExtension(lnk)).Contains(n));
                if (!byTarget && !byName) continue;
                if (byTarget && File.Exists(target)) continue; // the program is still there — keep its shortcut
                result.Add(new Leftover { Kind = LeftoverKind.Shortcut, Target = lnk, Title = Path.GetFileNameWithoutExtension(lnk) });
            }
            // an empty Start menu folder named after the program
            foreach (var n in names)
            {
                string dir = Path.Combine(root, n);
                if (Directory.Exists(dir) && !Directory.EnumerateFileSystemEntries(dir).Any())
                    result.Add(new Leftover { Kind = LeftoverKind.Folder, Target = dir, Title = n + " (пустая папка меню «Пуск»)" });
            }
        }
    }

    // ------------------------------------------------------------------ registry

    private static void FindRegistry(InstalledApp app, List<string> names, string publisher, HashSet<string> otherNames, List<Leftover> result)
    {
        var bases = new List<(RegistryHive hive, RegistryView view, string root, string prefix, bool admin)>
        {
            (RegistryHive.CurrentUser, RegistryView.Registry64, "Software", "HKEY_CURRENT_USER\\Software", false),
            (RegistryHive.LocalMachine, RegistryView.Registry64, "SOFTWARE", "HKEY_LOCAL_MACHINE\\SOFTWARE", true),
        };
        if (Environment.Is64BitOperatingSystem)
            bases.Add((RegistryHive.LocalMachine, RegistryView.Registry32, "SOFTWARE", "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node", true));

        var publishers = new List<string>();
        if (!string.IsNullOrEmpty(publisher)) publishers.Add(publisher);
        if (!string.IsNullOrWhiteSpace(app.Publisher) && !publishers.Contains(app.Publisher.Trim(), StringComparer.OrdinalIgnoreCase))
            publishers.Add(app.Publisher.Trim());

        foreach (var (hive, view, root, prefix, admin) in bases)
        {
            using var b = RegistryKey.OpenBaseKey(hive, view);
            using var software = b.OpenSubKey(root);
            if (software == null) continue;
            foreach (var n in names)
            {
                if (SharedNames.Contains(n) || otherNames.Contains(n)) continue;
                // Software\<Name>
                if (KeyExists(software, n))
                    Add(result, $"{prefix}\\{n}", n, admin);
                // Software\<Publisher>\<Name>; the publisher key too when nothing else is inside
                foreach (var pub in publishers)
                {
                    if (SharedNames.Contains(pub) || string.Equals(pub, n, StringComparison.OrdinalIgnoreCase)) continue;
                    using var pk = software.OpenSubKey(pub);
                    if (pk == null) continue;
                    string[] subs;
                    try { subs = pk.GetSubKeyNames(); }
                    catch { continue; }
                    if (!subs.Contains(n, StringComparer.OrdinalIgnoreCase)) continue;
                    bool onlyThis = subs.Length == 1 && pk.ValueCount == 0 && !otherNames.Contains(pub);
                    if (onlyThis) Add(result, $"{prefix}\\{pub}", pub, admin);
                    else Add(result, $"{prefix}\\{pub}\\{n}", $"{pub}\\{n}", admin);
                }
            }
        }
    }

    private static bool KeyExists(RegistryKey parent, string name)
    {
        try
        {
            using var k = parent.OpenSubKey(name);
            return k != null;
        }
        catch
        {
            return false;
        }
    }

    private static void Add(List<Leftover> result, string fullKey, string title, bool admin)
    {
        if (result.Any(r => r.Kind == LeftoverKind.RegistryKey &&
                            (IsSameOrParent(r.Target, fullKey) || IsSameOrParent(fullKey, r.Target)))) return;
        result.Add(new Leftover { Kind = LeftoverKind.RegistryKey, Target = fullKey, Title = title, NeedsAdmin = admin });
    }

    // ------------------------------------------------------------------ autostart / tasks / services / firewall

    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";

    private static void FindStartup(Func<string, bool> mine, List<Leftover> result)
    {
        var places = new (RegistryHive hive, RegistryView view, string prefix, bool admin)[]
        {
            (RegistryHive.CurrentUser, RegistryView.Registry64, "HKEY_CURRENT_USER", false),
            (RegistryHive.LocalMachine, RegistryView.Registry64, "HKEY_LOCAL_MACHINE", true),
            (RegistryHive.LocalMachine, RegistryView.Registry32, "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node", true),
        };
        foreach (var (hive, view, prefix, admin) in places)
        {
            using var b = RegistryKey.OpenBaseKey(hive, view);
            using var k = b.OpenSubKey(RunKey);
            if (k == null) continue;
            foreach (var name in k.GetValueNames())
            {
                string exe = CommandLine.ExtractExecutable(k.GetValue(name) as string) ?? RawExe(k.GetValue(name) as string);
                if (exe == null || !mine(exe) || File.Exists(exe)) continue;
                string keyPath = view == RegistryView.Registry32 ? $"{prefix}\\Microsoft\\Windows\\CurrentVersion\\Run" : $"{prefix}\\{RunKey}";
                result.Add(new Leftover { Kind = LeftoverKind.StartupEntry, Target = keyPath, ValueName = name, Title = name, NeedsAdmin = admin });
            }
        }
    }

    private static string RawExe(string cmd)
    {
        if (string.IsNullOrWhiteSpace(cmd)) return null;
        cmd = cmd.Trim();
        if (cmd.StartsWith('"'))
        {
            int end = cmd.IndexOf('"', 1);
            return end > 1 ? cmd.Substring(1, end - 1) : null;
        }
        int exe = cmd.IndexOf(".exe", StringComparison.OrdinalIgnoreCase);
        return exe > 0 ? cmd.Substring(0, exe + 4) : null;
    }

    private static void FindTasks(Func<string, bool> mine, List<Leftover> result)
    {
        foreach (var t in TaskSchedulerHelper.ListAll())
        {
            if (string.IsNullOrEmpty(t.Command) || !mine(t.Command) || File.Exists(t.Command)) continue;
            if (t.Path.StartsWith(@"\Prostor\", StringComparison.OrdinalIgnoreCase)) continue;
            bool own = t.UserId != null && t.UserId.EndsWith(Environment.UserName, StringComparison.OrdinalIgnoreCase);
            result.Add(new Leftover { Kind = LeftoverKind.Task, Target = t.Path, Title = t.Name, NeedsAdmin = !own });
        }
    }

    private static void FindServices(Func<string, bool> mine, List<Leftover> result)
    {
        using var root = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services");
        if (root == null) return;
        foreach (var name in root.GetSubKeyNames())
        {
            try
            {
                using var k = root.OpenSubKey(name);
                string image = k?.GetValue("ImagePath") as string;
                if (string.IsNullOrWhiteSpace(image)) continue;
                string exe = RawExe(Environment.ExpandEnvironmentVariables(image));
                if (exe == null || !mine(exe) || File.Exists(exe)) continue;
                string display = k.GetValue("DisplayName") as string ?? name;
                result.Add(new Leftover { Kind = LeftoverKind.Service, Target = name, Title = display, NeedsAdmin = true });
            }
            catch { }
        }
    }

    private const string FirewallKey = @"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules";

    private static void FindFirewall(Func<string, bool> mine, string location, List<Leftover> result)
    {
        using var k = Registry.LocalMachine.OpenSubKey(FirewallKey);
        if (k == null) return;
        var programs = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var v in k.GetValueNames())
        {
            if (k.GetValue(v) is not string rule) continue;
            int i = rule.IndexOf("|App=", StringComparison.OrdinalIgnoreCase);
            if (i < 0) continue;
            int end = rule.IndexOf('|', i + 5);
            string app = Environment.ExpandEnvironmentVariables(end < 0 ? rule[(i + 5)..] : rule[(i + 5)..end]);
            if (mine(app) && !File.Exists(app)) programs.Add(app);
        }
        foreach (var p in programs)
            result.Add(new Leftover { Kind = LeftoverKind.FirewallRule, Target = p, Title = "Правила для " + Path.GetFileName(p), NeedsAdmin = true });
    }

    // ------------------------------------------------------------------ removal

    public static LeftoverRemoval Remove(IEnumerable<Leftover> items, string label, IProgress<string> status = null)
    {
        var r = new LeftoverRemoval();
        foreach (var item in items)
        {
            status?.Report(item.Target);
            bool ok;
            try
            {
                ok = RemoveOne(item, label, r);
            }
            catch
            {
                ok = false;
            }
            if (ok) r.Removed++;
            else r.Failed++;
        }
        return r;
    }

    private static bool RemoveOne(Leftover item, string label, LeftoverRemoval r)
    {
        switch (item.Kind)
        {
            case LeftoverKind.Folder:
            case LeftoverKind.Shortcut:
                if (AppSettings.Current.IsExcluded(item.Target)) return false;
                if (!ShellHelper.MoveToRecycleBin(item.Target)) return false;
                r.Recycled.Add(item.Target);
                r.Bytes += item.Size;
                return true;

            case LeftoverKind.RegistryKey:
            {
                string backup = RegistryCleaner.Backup(item.Target, label + "_" + item.Title);
                if (backup == null) return false; // never delete what could not be backed up
                if (!DeleteKey(item.Target)) return false;
                r.RegBackups.Add(backup);
                return true;
            }

            case LeftoverKind.StartupEntry:
            {
                string backup = RegistryCleaner.Backup(item.Target, label + "_Run");
                var (hive, view, sub) = SplitKey(item.Target);
                using var b = RegistryKey.OpenBaseKey(hive, view);
                using var k = b.OpenSubKey(sub, true);
                if (k == null) return false;
                k.DeleteValue(item.ValueName, false);
                if (backup != null) r.RegBackups.Add(backup);
                return true;
            }

            case LeftoverKind.Task:
            {
                string xml = TaskSchedulerHelper.ExportXml(item.Target);
                Directory.CreateDirectory(RegistryCleaner.BackupFolder);
                string file = Path.Combine(RegistryCleaner.BackupFolder, $"task_{Safe(item.Title)}_{DateTime.Now:yyyyMMdd_HHmmss}.xml");
                File.WriteAllText(file, xml);
                TaskSchedulerHelper.Delete(item.Target);
                if (TaskSchedulerHelper.Exists(item.Target)) return false;
                r.TaskBackups.Add(item.Target + "|" + file);
                return true;
            }

            case LeftoverKind.Service:
            {
                string backup = RegistryCleaner.Backup($@"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\{item.Target}", label + "_service");
                Cmd.Run(Cmd.SystemTool("sc.exe"), $"stop {Cmd.Quote(item.Target)}", timeoutMs: 20_000);
                var res = Cmd.Run(Cmd.SystemTool("sc.exe"), $"delete {Cmd.Quote(item.Target)}", timeoutMs: 20_000);
                if (!res.Ok) return false;
                if (backup != null) r.RegBackups.Add(backup);
                return true;
            }

            case LeftoverKind.FirewallRule:
            {
                var res = Cmd.Run(Cmd.SystemTool("netsh.exe"), $"advfirewall firewall delete rule name=all program={Cmd.Quote(item.Target)}", timeoutMs: 30_000);
                return res.Ok;
            }
        }
        return false;
    }

    private static string Safe(string s) => string.Concat((s ?? "").Select(c => Path.GetInvalidFileNameChars().Contains(c) ? '_' : c));

    private static (RegistryHive hive, RegistryView view, string sub) SplitKey(string full)
    {
        RegistryHive hive = full.StartsWith("HKEY_CURRENT_USER", StringComparison.OrdinalIgnoreCase) ? RegistryHive.CurrentUser : RegistryHive.LocalMachine;
        string sub = full.Substring(full.IndexOf('\\') + 1);
        var view = RegistryView.Registry64;
        if (sub.StartsWith(@"SOFTWARE\WOW6432Node\", StringComparison.OrdinalIgnoreCase))
        {
            view = RegistryView.Registry32;
            sub = @"SOFTWARE\" + sub.Substring(@"SOFTWARE\WOW6432Node\".Length);
        }
        return (hive, view, sub);
    }

    private static bool DeleteKey(string full)
    {
        var (hive, view, sub) = SplitKey(full);
        using var b = RegistryKey.OpenBaseKey(hive, view);
        b.DeleteSubKeyTree(sub, false);
        using var check = b.OpenSubKey(sub);
        return check == null;
    }

    // ------------------------------------------------------------------ names

    private static bool IsSameOrParent(string parent, string child) =>
        string.Equals(parent.TrimEnd('\\'), child.TrimEnd('\\'), StringComparison.OrdinalIgnoreCase) ||
        child.StartsWith(parent.TrimEnd('\\') + "\\", StringComparison.OrdinalIgnoreCase);

    private static string Normalize(string s) => new string((s ?? "").Where(char.IsLetterOrDigit).ToArray()).ToLowerInvariant();

    public static List<string> CleanNames(string name)
    {
        var list = new List<string>();
        if (string.IsNullOrWhiteSpace(name)) return list;
        string n = ParensRx.Replace(name, "");
        n = ArchRx.Replace(n, "");
        n = VersionRx.Replace(n, "");
        n = n.Trim(' ', '-', '–', '_', '.', ',');
        if (n.Length >= 3) list.Add(n);
        string noSpaces = n.Replace(" ", "");
        if (noSpaces.Length >= 3 && noSpaces != n) list.Add(noSpaces);
        return list;
    }

    public static string CleanPublisher(string publisher)
    {
        if (string.IsNullOrWhiteSpace(publisher)) return null;
        string p = CompanyRx.Replace(publisher, "").Trim(' ', ',', '.', '-');
        return p.Length >= 2 ? p : null;
    }

    /// <summary>Processes started from the program folder (to close them before a forced removal).</summary>
    public static List<Process> RunningFrom(string folder)
    {
        var list = new List<Process>();
        if (string.IsNullOrEmpty(folder)) return list;
        foreach (var p in Process.GetProcesses())
        {
            try
            {
                string path = p.MainModule?.FileName;
                if (path != null && IsSameOrParent(folder, path)) list.Add(p);
                else p.Dispose();
            }
            catch
            {
                p.Dispose();
            }
        }
        return list;
    }
}
