using System.Globalization;
using System.Text.RegularExpressions;
using Microsoft.Win32;
using Prostor.Core;
using Prostor.Models;
using Windows.Management.Deployment;

namespace Prostor.Services;

/// <summary>Collects installed desktop programs (registry) and Microsoft Store apps.</summary>
public static class AppInventory
{
    public const string UninstallKey = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    private static readonly string[] HiddenStorePrefixes =
    {
        "Microsoft.WindowsStore", "Microsoft.DesktopAppInstaller", "Microsoft.StorePurchaseApp", "Microsoft.SecHealthUI",
        "Microsoft.Windows.", "MicrosoftWindows.", "Windows.", "Microsoft.AAD.", "Microsoft.AccountsControl",
        "Microsoft.LockApp", "Microsoft.ECApp", "Microsoft.CredDialogHost", "Microsoft.Win32WebViewHost",
        "Microsoft.XboxGameCallableUI", "NcsiUwpApp", "Microsoft.AsyncTextService", "Microsoft.BioEnrollment",
        "Microsoft.MicrosoftEdgeDevToolsClient", "Microsoft.WindowsAppRuntime", "MicrosoftCorporationII.WinAppRuntime",
        "Microsoft.WinAppRuntime", "Microsoft.ApplicationCompatibilityEnhancements", "Microsoft.VCLibs", "Microsoft.UI.Xaml",
        "Microsoft.NET.", "Microsoft.Services.Store", "Microsoft.Advertising", "Microsoft.Ink.", "Microsoft.WidgetsPlatformRuntime",
        "Microsoft.Winget", "Microsoft.OneDriveSync", "c5e2524a-ea46-4f67-841f-6a9465d9d515", "1527c705-839a-4832-9118-54d4Bd6a0c89",
        "E2A4F912-2574-4A75-9BB0-0D023378592B", "F46D4000-FD22-4DB4-AC8E-4E1DDDE828FE", "windows.immersivecontrolpanel"
    };

    public static List<InstalledApp> LoadAll()
    {
        var list = LoadDesktop();
        try
        {
            list.AddRange(LoadStore());
        }
        catch
        {
            // Store inventory is optional (e.g. on stripped-down systems)
        }
        GameLaunchers.TagXbox(list);
        foreach (var app in list) app.Hint = AppAdvisor.Classify(app);
        return list;
    }

    // ---------------------------------------------------------------- desktop programs

    public static List<InstalledApp> LoadDesktop()
    {
        var result = new List<InstalledApp>();
        var seenKeys = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        Read(RegistryHive.LocalMachine, RegistryView.Registry64, result, seenKeys);
        if (Environment.Is64BitOperatingSystem)
            Read(RegistryHive.LocalMachine, RegistryView.Registry32, result, seenKeys);
        Read(RegistryHive.CurrentUser, RegistryView.Registry64, result, seenKeys);
        Read(RegistryHive.CurrentUser, RegistryView.Registry32, result, seenKeys);

        // The same product can be registered several times (per-view / per-user): keep the richest entry.
        var list = result
            .GroupBy(a => (a.Name.ToLowerInvariant(), (a.Version ?? "").ToLowerInvariant()))
            .Select(g => g.OrderByDescending(a => a.SizeBytes).ThenByDescending(a => a.HasLocation).First())
            .ToList();
        SteamLibrary.Attach(list);
        GameLaunchers.Attach(list);
        return list;
    }

    private static void Read(RegistryHive hive, RegistryView view, List<InstalledApp> result, HashSet<string> seen)
    {
        try
        {
            using var baseKey = RegistryKey.OpenBaseKey(hive, view);
            using var root = baseKey.OpenSubKey(UninstallKey);
            if (root == null) return;
            foreach (var keyName in root.GetSubKeyNames())
            {
                string seenId = hive == RegistryHive.CurrentUser ? "HKCU|" + keyName : $"HKLM|{view}|{keyName}";
                if (!seen.Add(seenId)) continue;
                try
                {
                    using var k = root.OpenSubKey(keyName);
                    if (k == null) continue;
                    var app = FromKey(k, keyName, hive, view);
                    if (app != null) result.Add(app);
                }
                catch { }
            }
        }
        catch { }
    }

    private static InstalledApp FromKey(RegistryKey k, string keyName, RegistryHive hive, RegistryView view)
    {
        string name = (k.GetValue("DisplayName") as string)?.Trim();
        if (string.IsNullOrEmpty(name)) return null;
        if (GetInt(k, "SystemComponent") == 1) return null;
        if (!string.IsNullOrEmpty(k.GetValue("ParentKeyName") as string)) return null;
        string releaseType = k.GetValue("ReleaseType") as string;
        if (releaseType is "Update" or "Hotfix" or "Security Update" or "Service Pack") return null;
        if (Regex.IsMatch(name, @"^(Security )?Update for |\(KB\d{6,}\)")) return null;

        string uninstall = (k.GetValue("UninstallString") as string)?.Trim();
        bool isMsi = GetInt(k, "WindowsInstaller") == 1 && Guid.TryParse(keyName, out _);
        if (string.IsNullOrWhiteSpace(uninstall) && !isMsi) return null;

        string location = Clean(k.GetValue("InstallLocation") as string);
        string icon = Clean(k.GetValue("DisplayIcon") as string);
        if (!IsUsableLocation(location)) location = GuessLocation(icon, uninstall);

        DateTime? date = ParseDate(k.GetValue("InstallDate") as string);
        if (date == null)
        {
            try
            {
                if (Native.RegQueryInfoKey(k.Handle, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero,
                        IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, out long ft) == 0 && ft > 0)
                    date = DateTime.FromFileTime(ft);
            }
            catch { }
        }

        long sizeKb = GetLong(k, "EstimatedSize");

        var app = new InstalledApp
        {
            Id = $"{hive}|{view}|{keyName}",
            Name = name,
            Publisher = (k.GetValue("Publisher") as string)?.Trim(),
            Version = (k.GetValue("DisplayVersion") as string)?.Trim(),
            InstallDate = date,
            InstallLocation = location,
            UninstallString = uninstall,
            QuietUninstallString = (k.GetValue("QuietUninstallString") as string)?.Trim(),
            ModifyPath = GetInt(k, "NoModify") == 1 ? null : (k.GetValue("ModifyPath") as string)?.Trim(),
            DisplayIcon = icon,
            HelpLink = (k.GetValue("URLInfoAbout") as string ?? k.GetValue("HelpLink") as string)?.Trim(),
            IsMsi = isMsi,
            ProductCode = isMsi ? keyName : null,
            NoRemove = GetInt(k, "NoRemove") == 1,
            Hive = hive,
            View = view,
            RegistrySubKey = UninstallKey + "\\" + keyName,
        };
        if (sizeKb > 0) app.SizeBytes = sizeKb * 1024;
        return app;
    }

    private static string Clean(string s)
    {
        if (string.IsNullOrWhiteSpace(s)) return null;
        s = Environment.ExpandEnvironmentVariables(s.Trim().Trim('"').Trim());
        return s.Length == 0 ? null : s;
    }

    private static int GetInt(RegistryKey k, string name) => (int)GetLong(k, name);

    private static long GetLong(RegistryKey k, string name)
    {
        var v = k.GetValue(name);
        return v switch
        {
            int i => (uint)i,
            long l => l,
            string s when long.TryParse(s, out var r) => r,
            _ => 0
        };
    }

    private static DateTime? ParseDate(string s)
    {
        if (string.IsNullOrWhiteSpace(s)) return null;
        string[] formats = { "yyyyMMdd", "yyyy-MM-dd", "yyyy.MM.dd", "dd.MM.yyyy", "M/d/yyyy", "MM/dd/yyyy", "d/M/yyyy", "yyyy/MM/dd" };
        if (DateTime.TryParseExact(s.Trim(), formats, CultureInfo.InvariantCulture, DateTimeStyles.None, out var d) &&
            d.Year >= 1995 && d <= DateTime.Now.AddDays(1))
            return d;
        return null;
    }

    private static readonly string[] Protected = BuildProtected();

    private static string[] BuildProtected()
    {
        var list = new List<string>();
        void Add(string p)
        {
            if (!string.IsNullOrEmpty(p)) list.Add(p.TrimEnd('\\'));
        }
        Add(Environment.GetFolderPath(Environment.SpecialFolder.Windows));
        Add(Environment.SystemDirectory);
        Add(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.CommonProgramFiles));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.CommonProgramFilesX86));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments));
        Add(Environment.GetFolderPath(Environment.SpecialFolder.Desktop));
        Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs"));
        Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "AppData"));
        Add(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), @"AppData\LocalLow"));
        return list.ToArray();
    }

    /// <summary>True for a real program folder (not a drive root or a well-known system folder).</summary>
    public static bool IsUsableLocation(string path) => IsUsableLocationPath(path) && Directory.Exists(path);

    /// <summary>Same checks as <see cref="IsUsableLocation"/> without requiring the folder to exist (it may be gone after an uninstall).</summary>
    public static bool IsUsableLocationPath(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return false;
        try
        {
            if (!Path.IsPathFullyQualified(path)) return false;
            string full = Path.GetFullPath(path).TrimEnd('\\');
            if (full.Length <= 3) return false;
            if (Protected.Any(p => string.Equals(p, full, StringComparison.OrdinalIgnoreCase))) return false;
            string win = Environment.GetFolderPath(Environment.SpecialFolder.Windows);
            if (full.StartsWith(win + "\\", StringComparison.OrdinalIgnoreCase)) return false;
            if (full.Contains(@"\Package Cache\", StringComparison.OrdinalIgnoreCase)) return false;
            if (full.Contains(@"\Installer\", StringComparison.OrdinalIgnoreCase)) return false;
            return true;
        }
        catch
        {
            return false;
        }
    }

    private static string GuessLocation(string icon, string uninstall)
    {
        foreach (var candidate in new[] { IconFile(icon), CommandLine.ExtractExecutable(uninstall) })
        {
            if (string.IsNullOrEmpty(candidate)) continue;
            try
            {
                string dir = Path.GetDirectoryName(candidate);
                if (IsUsableLocation(dir)) return dir;
            }
            catch { }
        }
        return null;
    }

    private static string IconFile(string icon)
    {
        if (string.IsNullOrEmpty(icon)) return null;
        string s = icon;
        int comma = s.LastIndexOf(',');
        if (comma > 2) s = s.Substring(0, comma);
        s = s.Trim().Trim('"');
        return File.Exists(s) ? s : null;
    }

    // ---------------------------------------------------------------- Microsoft Store

    public static List<InstalledApp> LoadStore()
    {
        var list = new List<InstalledApp>();
        var pm = new PackageManager();
        foreach (var p in pm.FindPackagesForUser(string.Empty))
        {
            try
            {
                if (p.IsFramework || p.IsResourcePackage || p.IsBundle) continue;
                if (p.SignatureKind == Windows.ApplicationModel.PackageSignatureKind.System) continue;
                string id = p.Id.Name;
                if (HiddenStorePrefixes.Any(x => id.StartsWith(x, StringComparison.OrdinalIgnoreCase))) continue;

                string name = null;
                try { name = p.DisplayName; } catch { }
                if (string.IsNullOrWhiteSpace(name) || name.StartsWith("ms-resource:", StringComparison.OrdinalIgnoreCase)) continue;

                string location = null;
                try { location = p.InstalledPath; } catch { }
                if (string.IsNullOrEmpty(location))
                {
                    try { location = p.InstalledLocation?.Path; } catch { }
                }

                string publisher = null;
                try { publisher = p.PublisherDisplayName; } catch { }

                DateTime? date = null;
                try
                {
                    var dt = p.InstalledDate;
                    if (dt.Year > 2000) date = dt.LocalDateTime;
                }
                catch { }

                string logo = null;
                try { logo = ResolveLogo(p.Logo); } catch { }

                var v = p.Id.Version;
                list.Add(new InstalledApp
                {
                    Id = "store|" + p.Id.FullName,
                    Name = name.Trim(),
                    Publisher = publisher,
                    Version = $"{v.Major}.{v.Minor}.{v.Build}.{v.Revision}",
                    InstallDate = date,
                    InstallLocation = location,
                    IsStore = true,
                    PackageFullName = p.Id.FullName,
                    PackageFamilyName = p.Id.FamilyName,
                    LogoPath = logo,
                });
            }
            catch { }
        }
        return list;
    }

    private static string ResolveLogo(Uri logo)
    {
        if (logo == null) return null;
        string path = logo.LocalPath;
        if (File.Exists(path)) return path;
        string dir = Path.GetDirectoryName(path);
        if (dir == null || !Directory.Exists(dir)) return null;
        string name = Path.GetFileNameWithoutExtension(path);
        string ext = Path.GetExtension(path);
        var files = Directory.GetFiles(dir, name + ".*" + ext)
            .Where(f => !f.Contains("contrast", StringComparison.OrdinalIgnoreCase))
            .ToList();
        if (files.Count == 0) return null;
        string Pick(string token) => files.FirstOrDefault(f => f.Contains(token, StringComparison.OrdinalIgnoreCase));
        return Pick("scale-200") ?? Pick("scale-150") ?? Pick("targetsize-48") ?? Pick("scale-100") ?? files[0];
    }

    /// <summary>Folder size used when the installer did not report one.</summary>
    /// <param name="otherLocations">Install folders of the other programs; nested ones are not counted twice.</param>
    public static long MeasureSize(InstalledApp app, ISet<string> otherLocations = null, CancellationToken ct = default)
    {
        long total = 0;
        if (!string.IsNullOrEmpty(app.InstallLocation) && Directory.Exists(app.InstallLocation))
        {
            string own = app.InstallLocation.TrimEnd('\\');
            ISet<string> exclude = null;
            if (otherLocations != null)
            {
                exclude = new HashSet<string>(
                    otherLocations.Where(o => o.StartsWith(own + "\\", StringComparison.OrdinalIgnoreCase)),
                    StringComparer.OrdinalIgnoreCase);
                if (exclude.Count == 0) exclude = null;
            }
            total += FastDir.GetSize(own, ct, out _, exclude);
        }
        foreach (var extra in app.ExtraLocations)
            if (Directory.Exists(extra)) total += FastDir.GetSize(extra, ct);
        if (app.IsStore && !string.IsNullOrEmpty(app.PackageFamilyName))
        {
            string data = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Packages", app.PackageFamilyName);
            if (Directory.Exists(data)) total += FastDir.GetSize(data, ct);
        }
        return total;
    }

    /// <summary>Best icon for the program, or null.</summary>
    public static System.Windows.Media.ImageSource LoadIcon(InstalledApp app)
    {
        if (app.IsStore)
            return string.IsNullOrEmpty(app.LogoPath) ? null : IconLoader.FromImage(app.LogoPath, 64);

        var img = IconLoader.FromIconSpec(app.DisplayIcon);
        if (img != null) return img;

        if (!string.IsNullOrEmpty(app.InstallLocation))
        {
            try
            {
                var exes = Directory.GetFiles(app.InstallLocation, "*.exe", SearchOption.TopDirectoryOnly)
                    .Where(f =>
                    {
                        string n = Path.GetFileName(f).ToLowerInvariant();
                        return !n.Contains("unins") && !n.Contains("uninst") && !n.Contains("update") && !n.Contains("crash") &&
                               !n.Contains("helper") && !n.Contains("setup");
                    })
                    .ToList();
                string key = new string(app.Name.Where(char.IsLetterOrDigit).Take(6).ToArray()).ToLowerInvariant();
                string best = exes.FirstOrDefault(f => key.Length > 2 && Path.GetFileNameWithoutExtension(f).ToLowerInvariant().Contains(key))
                              ?? exes.OrderByDescending(f => new FileInfo(f).Length).FirstOrDefault();
                if (best != null)
                {
                    img = IconLoader.FromFile(best);
                    if (img != null) return img;
                }
            }
            catch { }
        }

        string uninstallExe = CommandLine.ExtractExecutable(app.UninstallString);
        if (uninstallExe != null && !uninstallExe.EndsWith("msiexec.exe", StringComparison.OrdinalIgnoreCase) &&
            !uninstallExe.EndsWith("rundll32.exe", StringComparison.OrdinalIgnoreCase))
            return IconLoader.FromFile(uninstallExe);
        return null;
    }
}
