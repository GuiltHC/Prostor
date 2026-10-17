using System.ComponentModel;
using System.Diagnostics;
using System.Text.RegularExpressions;
using Windows.Foundation;
using Microsoft.Win32;
using Prostor.Core;
using Prostor.Models;
using Windows.Management.Deployment;

namespace Prostor.Services;

public static class Uninstaller
{
    /// <summary>Launches the program's own uninstaller. UAC is handled by the shell.</summary>
    public static Process Start(InstalledApp app, bool quiet)
    {
        if (app.LauncherManifest != null)
        {
            // Epic has no uninstall command line: open the launcher's library, the user removes the game there
            Native.AllowSetForegroundWindow(Native.ASFW_ANY);
            Process.Start(new ProcessStartInfo(app.UninstallString) { UseShellExecute = true })?.Dispose();
            return null;
        }
        string cmd = quiet && !string.IsNullOrWhiteSpace(app.QuietUninstallString) ? app.QuietUninstallString : app.UninstallString;

        if (app.IsMsi && (string.IsNullOrWhiteSpace(cmd) || cmd.Contains("msiexec", StringComparison.OrdinalIgnoreCase)))
        {
            cmd = $"MsiExec.exe /X{app.ProductCode}" + (quiet ? " /qb /norestart" : "");
        }
        else if (cmd.Contains("msiexec", StringComparison.OrdinalIgnoreCase))
        {
            // "/I{GUID}" opens the maintenance UI — switch it to removal.
            cmd = Regex.Replace(cmd, @"/I\s*\{", "/X{", RegexOptions.IgnoreCase);
        }

        var (file, args) = CommandLine.Split(cmd);
        if (!file.Contains('\\') && !file.Contains(':') && CommandLine.ExtractExecutable(file) == null)
            throw new Win32Exception(2);
        if (Path.IsPathFullyQualified(file) && !File.Exists(file))
            throw new Win32Exception(2);

        var psi = new ProcessStartInfo(file, args) { UseShellExecute = true };
        try
        {
            string dir = Path.GetDirectoryName(file);
            if (!string.IsNullOrEmpty(dir) && Directory.Exists(dir)) psi.WorkingDirectory = dir;
        }
        catch { }

        // Let the uninstaller (or an already running launcher such as Steam) come to the foreground.
        Native.AllowSetForegroundWindow(Native.ASFW_ANY);
        return Process.Start(psi);
    }

    /// <summary>True when the uninstaller referenced by the entry no longer exists on disk.</summary>
    public static bool IsUninstallerMissing(InstalledApp app)
    {
        if (app.IsStore || app.IsMsi || app.IsLauncherUninstall || string.IsNullOrWhiteSpace(app.UninstallString)) return false;
        return CommandLine.ExtractExecutable(app.UninstallString) == null;
    }

    public static Process StartModify(InstalledApp app)
    {
        var (file, args) = CommandLine.Split(app.ModifyPath);
        return Process.Start(new ProcessStartInfo(file, args) { UseShellExecute = true });
    }

    public static bool IsStillInstalled(InstalledApp app)
    {
        try
        {
            if (app.IsStore)
            {
                var pm = new PackageManager();
                return pm.FindPackageForUser(string.Empty, app.PackageFullName) != null;
            }
            if (app.IsSteamGame && !string.IsNullOrEmpty(app.SteamManifest))
                return File.Exists(app.SteamManifest); // Steam deletes the manifest once the game is gone
            if (!string.IsNullOrEmpty(app.LauncherManifest))
                return File.Exists(app.LauncherManifest);
            if (!app.HasRegistryKey) return false;
            using var b = RegistryKey.OpenBaseKey(app.Hive, app.View);
            using var k = b.OpenSubKey(app.RegistrySubKey);
            return k != null;
        }
        catch
        {
            return true;
        }
    }

    /// <summary>
    /// Removes a Microsoft Store package. The result is decided by the real package state (polled), not only by the
    /// completion callback — some packages (e.g. WSA) finish removing without the callback ever reaching the app.
    /// </summary>
    public static Task RemoveStoreAppAsync(InstalledApp app, IProgress<double> progress, CancellationToken ct) => Task.Run(async () =>
    {
        var pm = new PackageManager();
        var op = pm.RemovePackageAsync(app.PackageFullName);
        op.Progress = (_, p) => progress?.Report(p.percentage / 100.0);
        var sw = Stopwatch.StartNew();
        while (true)
        {
            switch (op.Status)
            {
                case AsyncStatus.Completed:
                {
                    var r = op.GetResults();
                    if (r.ExtendedErrorCode != null && pm.FindPackageForUser(string.Empty, app.PackageFullName) != null)
                        throw new InvalidOperationException(string.IsNullOrWhiteSpace(r.ErrorText) ? r.ExtendedErrorCode.Message : r.ErrorText);
                    return;
                }
                case AsyncStatus.Error:
                    if (pm.FindPackageForUser(string.Empty, app.PackageFullName) == null) return;
                    throw new InvalidOperationException(op.ErrorCode?.Message ?? "Ошибка удаления пакета");
                case AsyncStatus.Canceled:
                    throw new OperationCanceledException();
            }
            if (sw.Elapsed > TimeSpan.FromSeconds(2) && pm.FindPackageForUser(string.Empty, app.PackageFullName) == null)
            {
                progress?.Report(1);
                return;
            }
            if (ct.IsCancellationRequested)
            {
                try { op.Cancel(); } catch { }
                throw new OperationCanceledException();
            }
            await Task.Delay(400);
        }
    });
}

/// <summary>
/// Follows an uninstaller together with the processes it spawns (NSIS / Inno Setup re-launch themselves from TEMP)
/// and brings its windows to the front once.
/// </summary>
public sealed class ProcessTracker
{
    private readonly HashSet<uint> _tracked = new();
    private readonly HashSet<uint> _dead = new();
    private readonly HashSet<IntPtr> _raised = new();
    private readonly HashSet<uint> _alive = new();

    public ProcessTracker(int rootPid) => _tracked.Add((uint)rootPid);

    public string ActiveName { get; private set; }

    /// <summary>Refreshes the process tree. Returns true while any process of the uninstaller is running.</summary>
    public bool Poll()
    {
        var entries = Snapshot();
        var present = new HashSet<uint>(entries.Select(e => e.pid));
        foreach (var pid in _tracked)
            if (!present.Contains(pid)) _dead.Add(pid);

        bool added;
        do
        {
            added = false;
            foreach (var e in entries)
            {
                if (_tracked.Contains(e.pid) || e.pid == e.parent) continue;
                // a parent PID that died and is present again was reused by an unrelated process
                if (_tracked.Contains(e.parent) && !(_dead.Contains(e.parent) && present.Contains(e.parent)))
                {
                    _tracked.Add(e.pid);
                    added = true;
                }
            }
        } while (added);

        _alive.Clear();
        ActiveName = null;
        foreach (var e in entries)
        {
            // a dead PID that shows up again belongs to an unrelated, reused process
            if (_tracked.Contains(e.pid) && !_dead.Contains(e.pid))
            {
                _alive.Add(e.pid);
                ActiveName ??= e.exe;
            }
        }
        return _alive.Count > 0;
    }

    public void RaiseWindows()
    {
        if (_alive.Count == 0) return;
        Native.EnumWindows((hwnd, _) =>
        {
            if (_raised.Contains(hwnd) || !Native.IsWindowVisible(hwnd) || Native.GetWindow(hwnd, Native.GW_OWNER) != IntPtr.Zero)
                return true;
            Native.GetWindowThreadProcessId(hwnd, out uint pid);
            if (!_alive.Contains(pid) || Native.GetWindowTextLength(hwnd) == 0) return true;
            _raised.Add(hwnd);
            if (Native.IsIconic(hwnd)) Native.ShowWindow(hwnd, Native.SW_RESTORE);
            Native.SetForegroundWindow(hwnd);
            return true;
        }, IntPtr.Zero);
    }

    private static List<(uint pid, uint parent, string exe)> Snapshot()
    {
        var list = new List<(uint, uint, string)>(256);
        IntPtr snap = Native.CreateToolhelp32Snapshot(Native.TH32CS_SNAPPROCESS, 0);
        if (snap == IntPtr.Zero || snap == Native.INVALID_HANDLE_VALUE) return list;
        try
        {
            var e = new Native.PROCESSENTRY32 { dwSize = (uint)System.Runtime.InteropServices.Marshal.SizeOf<Native.PROCESSENTRY32>() };
            if (Native.Process32First(snap, ref e))
            {
                do list.Add((e.th32ProcessID, e.th32ParentProcessID, e.szExeFile));
                while (Native.Process32Next(snap, ref e));
            }
        }
        finally
        {
            Native.CloseHandle(snap);
        }
        return list;
    }
}

/// <summary>Removes orphaned uninstall entries (the program is gone, its record stayed). A .reg backup is kept.</summary>
public static class RegistryCleaner
{
    public static string BackupFolder => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Prostor", "Backups");

    /// <summary>Deletes the uninstall entry. Returns the .reg backup (null when it could not be written).</summary>
    public static string RemoveUninstallEntry(InstalledApp app)
    {
        if (!app.HasRegistryKey) throw new InvalidOperationException("У этой программы нет записи в реестре.");
        string file = Backup(app.RegistryFullPath, app.Name);
        using var b = RegistryKey.OpenBaseKey(app.Hive, app.View);
        b.DeleteSubKeyTree(app.RegistrySubKey, false);
        return file;
    }

    /// <summary>Exports a key ("HKEY_CURRENT_USER\...") to the Backups folder.</summary>
    public static string Backup(string fullKeyPath, string label)
    {
        try
        {
            Directory.CreateDirectory(BackupFolder);
            string safe = string.Concat((label ?? "key").Select(c => Path.GetInvalidFileNameChars().Contains(c) ? '_' : c));
            if (safe.Length > 60) safe = safe[..60];
            string file = Path.Combine(BackupFolder, $"{safe}_{DateTime.Now:yyyyMMdd_HHmmss_fff}.reg");
            var r = Cmd.Run(Cmd.SystemTool("reg.exe"), $"export {Cmd.Quote(fullKeyPath)} {Cmd.Quote(file)} /y", timeoutMs: 20_000);
            return r.Ok && File.Exists(file) ? file : null;
        }
        catch
        {
            return null;
        }
    }
}
