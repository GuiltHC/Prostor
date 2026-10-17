using Microsoft.Win32;

namespace Prostor.Core;

/// <summary>"Анализировать в «Простор»" in the context menu of folders and drives (current user only).</summary>
public static class ExplorerIntegration
{
    private const string Verb = "Prostor.Analyze";
    private const string Title = "Анализировать в «Простор»";

    private static readonly string[] Roots =
    {
        @"Software\Classes\Directory\shell",
        @"Software\Classes\Directory\Background\shell",
        @"Software\Classes\Drive\shell",
    };

    private static string Exe => Environment.ProcessPath;

    public static bool IsEnabled
    {
        get
        {
            try
            {
                using var k = Registry.CurrentUser.OpenSubKey($@"{Roots[0]}\{Verb}\command");
                return k?.GetValue(null) is string cmd && cmd.Contains("--scan", StringComparison.OrdinalIgnoreCase);
            }
            catch
            {
                return false;
            }
        }
    }

    /// <summary>The menu points to a different copy of the program (it was moved or re-downloaded).</summary>
    public static bool PointsElsewhere
    {
        get
        {
            try
            {
                using var k = Registry.CurrentUser.OpenSubKey($@"{Roots[0]}\{Verb}\command");
                return k?.GetValue(null) is string cmd && !cmd.Contains(Exe, StringComparison.OrdinalIgnoreCase);
            }
            catch
            {
                return false;
            }
        }
    }

    public static void Enable()
    {
        foreach (var root in Roots)
        {
            using var k = Registry.CurrentUser.CreateSubKey($@"{root}\{Verb}");
            k.SetValue(null, Title);
            k.SetValue("Icon", $"\"{Exe}\",0");
            using var c = k.CreateSubKey("command");
            c.SetValue(null, $"\"{Exe}\" --scan \"%V\"");
        }
    }

    public static void Disable()
    {
        foreach (var root in Roots)
        {
            try { Registry.CurrentUser.DeleteSubKeyTree($@"{root}\{Verb}", false); }
            catch { }
        }
    }
}
