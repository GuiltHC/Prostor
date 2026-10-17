using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
using System.Xml.Linq;
using Microsoft.Win32;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

/// <summary>
/// Reads and toggles autostart entries: Run keys and Startup folders (the same way Task Manager does),
/// logon/boot tasks of Task Scheduler and third-party services.
/// </summary>
public static class StartupService
{
    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ApprovedRoot = @"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\";
    public const string DelayedFolder = @"\Prostor\Delayed";
    private const string KeyMarker = "Ключ: ";

    public static List<StartupItem> Load()
    {
        var list = new List<StartupItem>();
        ReadRun(list, RegistryHive.CurrentUser, RegistryView.Registry64, "Run", "Реестр · текущий пользователь");
        ReadRun(list, RegistryHive.LocalMachine, RegistryView.Registry64, "Run", "Реестр · все пользователи");
        if (Environment.Is64BitOperatingSystem)
            ReadRun(list, RegistryHive.LocalMachine, RegistryView.Registry32, "Run32", "Реестр · все пользователи (32-бит)");
        ReadFolder(list, Environment.GetFolderPath(Environment.SpecialFolder.Startup), RegistryHive.CurrentUser, "Папка автозагрузки");
        ReadFolder(list, Environment.GetFolderPath(Environment.SpecialFolder.CommonStartup), RegistryHive.LocalMachine, "Папка автозагрузки · все");

        List<ScheduledTaskInfo> tasks;
        try { tasks = TaskSchedulerHelper.ListAll(); }
        catch { tasks = new(); }
        AttachDelayed(list, tasks);
        ReadTasks(list, tasks);
        ReadServices(list);

        return list
            .OrderBy(i => i.Kind is StartupKind.Run or StartupKind.Folder ? 0 : i.Kind == StartupKind.Task ? 1 : 2)
            .ThenByDescending(i => i.IsEnabled)
            .ThenBy(i => i.Title, StringComparer.CurrentCultureIgnoreCase)
            .ToList();
    }

    private static void ReadRun(List<StartupItem> list, RegistryHive hive, RegistryView view, string approved, string source)
    {
        try
        {
            using var b = RegistryKey.OpenBaseKey(hive, view);
            using var k = b.OpenSubKey(RunKey);
            if (k == null) return;
            foreach (var name in k.GetValueNames())
            {
                if (string.IsNullOrEmpty(name)) continue;
                string cmd = k.GetValue(name)?.ToString();
                if (string.IsNullOrWhiteSpace(cmd)) continue;
                string exe = CommandLine.ExtractExecutable(cmd);
                var (company, desc) = VersionInfo(exe);
                list.Add(new StartupItem
                {
                    Kind = StartupKind.Run,
                    Name = name,
                    Command = cmd,
                    FilePath = exe,
                    Publisher = company,
                    Description = desc,
                    SourceText = source,
                    ApprovedHive = hive,
                    ApprovedKey = approved,
                    ApprovedValueName = name,
                    IsEnabled = ReadApproved(hive, approved, name)
                });
            }
        }
        catch { }
    }

    private static void ReadFolder(List<StartupItem> list, string folder, RegistryHive hive, string source)
    {
        try
        {
            if (string.IsNullOrEmpty(folder) || !Directory.Exists(folder)) return;
            foreach (var file in Directory.GetFiles(folder))
            {
                string fileName = Path.GetFileName(file);
                if (fileName.Equals("desktop.ini", StringComparison.OrdinalIgnoreCase)) continue;
                var (target, arguments) = file.EndsWith(".lnk", StringComparison.OrdinalIgnoreCase) ? ResolveShortcut(file) : (file, null);
                var (company, desc) = VersionInfo(target);
                list.Add(new StartupItem
                {
                    Kind = StartupKind.Folder,
                    Name = Path.GetFileNameWithoutExtension(file),
                    Command = target ?? file,
                    Arguments = arguments,
                    FilePath = target ?? file,
                    Publisher = company,
                    Description = desc,
                    SourceText = source,
                    ApprovedHive = hive,
                    ApprovedKey = "StartupFolder",
                    ApprovedValueName = fileName,
                    IsEnabled = ReadApproved(hive, "StartupFolder", fileName)
                });
            }
        }
        catch { }
    }

    private static void ReadTasks(List<StartupItem> list, List<ScheduledTaskInfo> tasks)
    {
        string me = TaskSchedulerHelper.CurrentUser;
        string shortMe = Environment.UserName;
        string win = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        foreach (var t in tasks)
        {
            if (!(t.AtLogon || t.AtBoot) || string.IsNullOrEmpty(t.Command)) continue;
            if (t.Path.StartsWith(@"\Prostor\", StringComparison.OrdinalIgnoreCase)) continue;
            string exe = ResolveExe(t.Command);
            var (company, desc) = VersionInfo(exe);
            if (company != null && company.Contains("Microsoft", StringComparison.OrdinalIgnoreCase) &&
                exe != null && exe.StartsWith(win, StringComparison.OrdinalIgnoreCase))
                continue; // part of Windows
            bool own = t.UserId != null && (t.UserId.Equals(me, StringComparison.OrdinalIgnoreCase) ||
                                            t.UserId.Equals(shortMe, StringComparison.OrdinalIgnoreCase) ||
                                            t.UserId.EndsWith("\\" + shortMe, StringComparison.OrdinalIgnoreCase));
            list.Add(new StartupItem
            {
                Kind = StartupKind.Task,
                Name = t.Name,
                Command = t.Command,
                Arguments = t.Arguments,
                FilePath = exe ?? t.Command,
                Publisher = company ?? (string.IsNullOrWhiteSpace(t.Author) || t.Author.StartsWith('$') ? null : t.Author),
                Description = desc,
                SourceText = t.AtBoot ? "Планировщик · при включении" : "Планировщик · при входе",
                TaskPath = t.Path,
                TaskOwnedByUser = own,
                IsEnabled = t.Enabled
            });
        }
    }

    /// <summary>Programs moved to a delayed start: their entry is off, a «Простор» task starts them a minute after logon.</summary>
    private static void AttachDelayed(List<StartupItem> list, List<ScheduledTaskInfo> tasks)
    {
        foreach (var t in tasks.Where(t => t.Path.StartsWith(DelayedFolder + "\\", StringComparison.OrdinalIgnoreCase)))
        {
            string key = DelayedKeyFromDescription(t.Description);
            var item = list.FirstOrDefault(i => i.Key == key);
            if (item == null || !t.Enabled) continue;
            item.DelayTaskPath = t.Path;
            item.IsEnabled = true;
        }
    }

    private static string DelayedKeyFromDescription(string description)
    {
        if (string.IsNullOrEmpty(description)) return null;
        int i = description.LastIndexOf(KeyMarker, StringComparison.Ordinal);
        return i < 0 ? null : description.Substring(i + KeyMarker.Length).Trim();
    }

    // ------------------------------------------------------------------ services

    private static void ReadServices(List<StartupItem> list)
    {
        string win = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
        var ours = new HashSet<string>(AppSettings.Current.ManualServices, StringComparer.OrdinalIgnoreCase);
        try
        {
            using var root = Registry.LocalMachine.OpenSubKey(@"SYSTEM\CurrentControlSet\Services");
            if (root == null) return;
            foreach (var name in root.GetSubKeyNames())
            {
                try
                {
                    using var k = root.OpenSubKey(name);
                    if (k == null) continue;
                    int type = k.GetValue("Type") is int t ? t : 0;
                    if ((type & 0x30) == 0) continue; // drivers
                    int start = k.GetValue("Start") is int s ? s : 3;
                    if (start != 2 && !(start == 3 && ours.Contains(name))) continue;
                    string image = k.GetValue("ImagePath") as string;
                    if (string.IsNullOrWhiteSpace(image)) continue;
                    string expanded = Environment.ExpandEnvironmentVariables(image);
                    string exe = CommandLine.ExtractExecutable(expanded) ?? ResolveExe(expanded);
                    if (exe == null) continue;
                    if (exe.StartsWith(win, StringComparison.OrdinalIgnoreCase)) continue; // Windows' own (svchost and friends)
                    var (company, desc) = VersionInfo(exe);
                    if (company != null && company.StartsWith("Microsoft", StringComparison.OrdinalIgnoreCase) &&
                        exe.Contains("Windows Defender", StringComparison.OrdinalIgnoreCase)) continue;
                    string display = ResolveIndirect(k.GetValue("DisplayName") as string) ?? name;
                    list.Add(new StartupItem
                    {
                        Kind = StartupKind.Service,
                        Name = display,
                        Command = image,
                        FilePath = exe,
                        Publisher = company,
                        Description = null,
                        SourceText = "Служба",
                        ServiceName = name,
                        ServiceDelayed = k.GetValue("DelayedAutostart") is int d && d == 1,
                        IsEnabled = start == 2
                    });
                }
                catch { }
            }
        }
        catch { }
    }

    private static string ResolveIndirect(string s)
    {
        if (string.IsNullOrEmpty(s) || !s.StartsWith('@')) return s;
        var sb = new StringBuilder(512);
        return SHLoadIndirectString(s, sb, sb.Capacity, IntPtr.Zero) == 0 ? sb.ToString() : null;
    }

    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
    private static extern int SHLoadIndirectString(string pszSource, StringBuilder pszOutBuf, int cchOutBuf, IntPtr ppvReserved);

    private static string ResolveExe(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return null;
        path = Environment.ExpandEnvironmentVariables(path.Trim().Trim('"'));
        try
        {
            if (File.Exists(path)) return Path.GetFullPath(path);
            if (!path.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) && File.Exists(path + ".exe")) return Path.GetFullPath(path + ".exe");
        }
        catch { }
        return CommandLine.ExtractExecutable(path);
    }

    // ------------------------------------------------------------------ toggling

    private static bool ReadApproved(RegistryHive hive, string sub, string valueName)
    {
        try
        {
            using var b = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
            using var k = b.OpenSubKey(ApprovedRoot + sub);
            if (k?.GetValue(valueName) is byte[] data && data.Length > 0)
                return (data[0] & 1) == 0; // 02/06 = enabled, 03/07 = disabled
        }
        catch { }
        return true;
    }

    public static void SetEnabled(StartupItem item, bool enabled)
    {
        switch (item.Kind)
        {
            case StartupKind.Task:
                TaskSchedulerHelper.SetEnabled(item.TaskPath, enabled);
                return;
            case StartupKind.Service:
            {
                string mode = enabled ? item.ServiceDelayed ? "delayed-auto" : "auto" : "demand";
                var r = Cmd.Run(Cmd.SystemTool("sc.exe"), $"config {Cmd.Quote(item.ServiceName)} start= {mode}");
                if (!r.Ok) throw new UnauthorizedAccessException(r.Output.Trim());
                var s = AppSettings.Current;
                s.ManualServices.RemoveAll(n => n.Equals(item.ServiceName, StringComparison.OrdinalIgnoreCase));
                if (!enabled) s.ManualServices.Add(item.ServiceName);
                s.Save();
                return;
            }
            default:
                if (!enabled && item.IsDelayed)
                {
                    TaskSchedulerHelper.Delete(item.DelayTaskPath);
                    item.DelayTaskPath = null;
                }
                SetApproved(item.ApprovedHive, item.ApprovedKey, item.ApprovedValueName, enabled);
                return;
        }
    }

    private static void SetApproved(RegistryHive hive, string approvedKey, string valueName, bool enabled)
    {
        using var b = RegistryKey.OpenBaseKey(hive, RegistryView.Registry64);
        using var k = b.CreateSubKey(ApprovedRoot + approvedKey, true) ?? throw new UnauthorizedAccessException();
        var data = new byte[12];
        data[0] = enabled ? (byte)0x02 : (byte)0x03;
        if (!enabled)
        {
            var ft = BitConverter.GetBytes(DateTime.Now.ToFileTime());
            Array.Copy(ft, 0, data, 4, 8);
        }
        k.SetValue(valueName, data, RegistryValueKind.Binary);
    }

    /// <summary>
    /// Starts the program a minute after logon from a «Простор» task and switches its own entry off.
    /// Returns the task path (for the journal).
    /// </summary>
    public static string MakeDelayed(StartupItem item, string delay = "PT1M")
    {
        string file, args;
        if (item.Kind == StartupKind.Folder)
        {
            file = item.FilePath;
            args = item.Arguments ?? "";
        }
        else
        {
            (file, args) = CommandLine.Split(item.Command);
        }
        if (string.IsNullOrEmpty(file)) throw new InvalidOperationException("Не удалось определить команду запуска.");
        string safe = string.Concat(item.Name.Select(c => char.IsLetterOrDigit(c) || c is ' ' or '-' or '_' or '.' ? c : '_')).Trim();
        if (safe.Length == 0) safe = "item";
        string path = $@"{DelayedFolder}\{safe}";
        string xml = TaskSchedulerHelper.LogonTaskXml(
            $"Простор: отложенный запуск «{item.Title}». {KeyMarker}{item.Key}", delay, file, args);
        TaskSchedulerHelper.Register(path, xml);
        try
        {
            SetApproved(item.ApprovedHive, item.ApprovedKey, item.ApprovedValueName, false);
        }
        catch
        {
            TaskSchedulerHelper.Delete(path);
            throw;
        }
        item.DelayTaskPath = path;
        return path;
    }

    /// <summary>Back to the normal start: removes the task and switches the original entry on.</summary>
    public static void UndoDelayed(string key, string taskPath)
    {
        TaskSchedulerHelper.Delete(taskPath);
        var parts = key.Split('|', 3);
        if (parts.Length == 3 && Enum.TryParse<RegistryHive>(parts[0], out var hive))
            SetApproved(hive, parts[1], parts[2], true);
    }

    // ------------------------------------------------------------------ boot time

    /// <summary>Boot durations recorded by Windows (the log needs administrator rights). Null when it is not readable.</summary>
    public static List<BootRecord> LoadBootHistory(int count = 20)
    {
        try
        {
            var r = Cmd.Run(Cmd.SystemTool("wevtutil.exe"),
                $"qe Microsoft-Windows-Diagnostics-Performance/Operational \"/q:*[System[(EventID=100)]]\" /f:xml /c:{count} /rd:true",
                Encoding.UTF8, 20_000);
            if (!r.Ok) return null;
            return ParseBootEvents(r.Output);
        }
        catch
        {
            return null;
        }
    }

    public static List<BootRecord> ParseBootEvents(string xml)
    {
        var list = new List<BootRecord>();
        if (string.IsNullOrWhiteSpace(xml) || !xml.Contains("<Event", StringComparison.Ordinal)) return list;
        var doc = XDocument.Parse("<root>" + xml + "</root>");
        XNamespace ns = "http://schemas.microsoft.com/win/2004/08/events/event";
        foreach (var ev in doc.Root!.Elements(ns + "Event"))
        {
            var data = ev.Element(ns + "EventData")?.Elements(ns + "Data")
                .GroupBy(d => (string)d.Attribute("Name") ?? "")
                .ToDictionary(g => g.Key, g => g.First().Value) ?? new();
            string time = (string)ev.Element(ns + "System")?.Element(ns + "TimeCreated")?.Attribute("SystemTime");
            if (!DateTime.TryParse(time, CultureInfo.InvariantCulture, DateTimeStyles.AdjustToUniversal | DateTimeStyles.AssumeUniversal, out var t))
                continue;
            double.TryParse(data.GetValueOrDefault("MainPathBootTime"), NumberStyles.Float, CultureInfo.InvariantCulture, out double main);
            double.TryParse(data.GetValueOrDefault("BootTime"), NumberStyles.Float, CultureInfo.InvariantCulture, out double total);
            int.TryParse(data.GetValueOrDefault("BootNumStartupApps"), out int apps);
            if (main <= 0 && total <= 0) continue;
            list.Add(new BootRecord
            {
                Time = DateTime.SpecifyKind(t, DateTimeKind.Utc).ToLocalTime(),
                MainPathSeconds = main / 1000, TotalSeconds = total / 1000, StartupApps = apps
            });
        }
        list.Sort((a, b) => a.Time.CompareTo(b.Time));
        return list;
    }

    // ------------------------------------------------------------------ helpers

    private static (string company, string description) VersionInfo(string path)
    {
        try
        {
            if (string.IsNullOrEmpty(path) || !File.Exists(path)) return (null, null);
            var vi = FileVersionInfo.GetVersionInfo(path);
            return (vi.CompanyName?.Trim(), vi.FileDescription?.Trim());
        }
        catch
        {
            return (null, null);
        }
    }

    private static (string target, string arguments) ResolveShortcut(string lnk)
    {
        try
        {
            var type = Type.GetTypeFromProgID("WScript.Shell");
            if (type == null) return (null, null);
            dynamic shell = Activator.CreateInstance(type);
            dynamic sc = shell.CreateShortcut(lnk);
            string target = sc.TargetPath;
            string args = sc.Arguments;
            return (string.IsNullOrWhiteSpace(target) ? null : target, string.IsNullOrWhiteSpace(args) ? null : args);
        }
        catch
        {
            return (null, null);
        }
    }

    public static System.Windows.Media.ImageSource LoadIcon(StartupItem item)
    {
        if (!string.IsNullOrEmpty(item.FilePath) && File.Exists(item.FilePath))
            return IconLoader.FromFile(item.FilePath, 48);
        return null;
    }
}

public static class DriveService
{
    public static List<DriveItem> GetDrives()
    {
        var list = new List<DriveItem>();
        foreach (var d in DriveInfo.GetDrives())
        {
            try
            {
                if (!d.IsReady) continue;
                if (d.DriveType != DriveType.Fixed && d.DriveType != DriveType.Removable) continue;
                if (d.TotalSize <= 0) continue;
                string label = string.IsNullOrWhiteSpace(d.VolumeLabel)
                    ? d.DriveType == DriveType.Removable ? "Съёмный диск" : "Локальный диск"
                    : d.VolumeLabel;
                list.Add(new DriveItem
                {
                    Name = d.Name.TrimEnd('\\'),
                    RootPath = d.Name,
                    Label = label,
                    Total = d.TotalSize,
                    Free = d.AvailableFreeSpace,
                    IsRemovable = d.DriveType == DriveType.Removable,
                    Format = d.DriveFormat
                });
            }
            catch { }
        }
        return list;
    }
}
