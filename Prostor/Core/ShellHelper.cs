using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Windows;
using Microsoft.Win32;

namespace Prostor.Core;

public static class ShellHelper
{
    private static readonly Lazy<bool> _isAdmin = new(() =>
    {
        try
        {
            using var id = WindowsIdentity.GetCurrent();
            return new WindowsPrincipal(id).IsInRole(WindowsBuiltInRole.Administrator);
        }
        catch
        {
            return false;
        }
    });

    public static bool IsAdmin => _isAdmin.Value;

    public static bool RestartAsAdmin(string page = null)
    {
        try
        {
            string args = "--new-window" + (string.IsNullOrEmpty(page) ? "" : $" --page {page}");
            Process.Start(new ProcessStartInfo(Environment.ProcessPath!, args) { UseShellExecute = true, Verb = "runas" });
            Application.Current.Shutdown();
            return true;
        }
        catch (Win32Exception)
        {
            return false; // the user declined the UAC prompt
        }
    }

    public static void OpenFolder(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return;
        try
        {
            if (Directory.Exists(path))
                Process.Start(new ProcessStartInfo("explorer.exe", $"\"{path}\"") { UseShellExecute = true });
            else
                ShowInExplorer(path);
        }
        catch { }
    }

    public static void ShowInExplorer(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return;
        try
        {
            if (File.Exists(path) || Directory.Exists(path))
                Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{path}\"") { UseShellExecute = true });
            else
            {
                var parent = Path.GetDirectoryName(path);
                if (parent != null && Directory.Exists(parent)) OpenFolder(parent);
            }
        }
        catch { }
    }

    public static void Open(string path)
    {
        try
        {
            Process.Start(new ProcessStartInfo(path) { UseShellExecute = true });
        }
        catch { }
    }

    public static void SearchWeb(string query)
    {
        if (string.IsNullOrWhiteSpace(query)) return;
        Open("https://www.google.com/search?q=" + Uri.EscapeDataString(query));
    }

    public static void CopyText(string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        for (int i = 0; i < 5; i++)
        {
            try
            {
                Clipboard.SetText(text);
                return;
            }
            catch (COMException)
            {
                Thread.Sleep(40);
            }
        }
    }

    /// <summary>Opens regedit positioned at the given key ("HKEY_LOCAL_MACHINE\...").</summary>
    public static void OpenRegedit(string fullKeyPath)
    {
        try
        {
            using (var k = Registry.CurrentUser.CreateSubKey(@"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit"))
                k?.SetValue("LastKey", fullKeyPath);
            Process.Start(new ProcessStartInfo("regedit.exe", "-m") { UseShellExecute = true });
        }
        catch { }
    }

    /// <summary>Moves a file or folder to the Recycle Bin. Returns true when the item no longer exists.</summary>
    public static bool MoveToRecycleBin(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return false;
        path = path.TrimEnd('\\');
        var op = new Native.SHFILEOPSTRUCT
        {
            wFunc = Native.FO_DELETE,
            pFrom = path + "\0",
            fFlags = (ushort)(Native.FOF_ALLOWUNDO | Native.FOF_NOCONFIRMATION | Native.FOF_WANTNUKEWARNING | Native.FOF_SILENT)
        };
        int rc = Native.SHFileOperation(ref op);
        if (rc != 0 || op.fAnyOperationsAborted) return !File.Exists(path) && !Directory.Exists(path);
        return !File.Exists(path) && !Directory.Exists(path);
    }

    public static (long size, long items) QueryRecycleBin()
    {
        var info = new Native.SHQUERYRBINFO { cbSize = Marshal.SizeOf<Native.SHQUERYRBINFO>() };
        int hr = Native.SHQueryRecycleBin(null, ref info);
        return hr == 0 ? (info.i64Size, info.i64NumItems) : (0, 0);
    }

    public static bool EmptyRecycleBin()
    {
        int hr = Native.SHEmptyRecycleBin(IntPtr.Zero, null,
            Native.SHERB_NOCONFIRMATION | Native.SHERB_NOPROGRESSUI | Native.SHERB_NOSOUND);
        return hr == 0 || hr == unchecked((int)0x8000FFFF); // E_UNEXPECTED is returned when already empty
    }

    public static string DownloadsFolder
    {
        get
        {
            try
            {
                if (Native.SHGetKnownFolderPath(Native.FOLDERID_Downloads, 0, IntPtr.Zero, out var p) == 0)
                {
                    string s = Marshal.PtrToStringUni(p);
                    Marshal.FreeCoTaskMem(p);
                    if (!string.IsNullOrEmpty(s)) return s;
                }
            }
            catch { }
            return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Downloads");
        }
    }

    public static string WindowsVersionText()
    {
        try
        {
            using var k = Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
            string product = k?.GetValue("ProductName") as string ?? "Windows";
            string display = k?.GetValue("DisplayVersion") as string;
            if (Environment.OSVersion.Version.Build >= 22000)
                product = product.Replace("Windows 10", "Windows 11");
            return string.IsNullOrEmpty(display) ? product : $"{product} {display}";
        }
        catch
        {
            return "Windows";
        }
    }
}
