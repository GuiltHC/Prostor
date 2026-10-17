using System.Windows;
using System.Windows.Media;
using Microsoft.Win32;

namespace Prostor.Core;

/// <summary>Light / dark / system theme, the system accent color and the Mica backdrop.</summary>
public static class ThemeManager
{
    public static event Action Changed;

    public static bool IsDark { get; private set; } = true;

    /// <summary>Mica needs Windows 11 22H2 or newer.</summary>
    public static bool MicaSupported => Environment.OSVersion.Version.Build >= 22621;

    public static bool MicaActive => MicaSupported && AppSettings.Current.UseMica && !SystemParameters.HighContrast;

    public static bool SystemIsDark
    {
        get
        {
            try
            {
                using var k = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
                return k?.GetValue("AppsUseLightTheme") is not int v || v == 0;
            }
            catch
            {
                return true;
            }
        }
    }

    public static void Apply()
    {
        var s = AppSettings.Current;
        bool dark = s.Theme switch { "light" => false, "dark" => true, _ => SystemIsDark };
        var dict = new ResourceDictionary
        {
            Source = new Uri($"pack://application:,,,/Themes/{(dark ? "Dark" : "Light")}.xaml", UriKind.Absolute)
        };

        if (s.UseSystemAccent && TryGetSystemAccent(dark, out var accent, out var hover))
        {
            dict["Brush.Accent"] = Brush(accent);
            dict["Brush.AccentHover"] = Brush(hover);
            dict["Brush.AccentSoft"] = Brush(Color.FromArgb(dark ? (byte)0x2A : (byte)0x1C, accent.R, accent.G, accent.B));
            dict["Brush.AccentBorder"] = Brush(Color.FromArgb(0x66, accent.R, accent.G, accent.B));
            dict["Brush.AccentText"] = Brush(Luminance(accent) > 0.55 ? Color.FromRgb(0x10, 0x12, 0x18) : Colors.White);
        }

        if (MicaActive)
        {
            // Mica shows through the navigation pane; the content stays opaque so the system caption
            // buttons that DWM draws under the extended frame never shine through.
            dict["Brush.Sidebar"] = Brush(Color.FromArgb(0x00, 0, 0, 0));
        }

        Application.Current.Resources.MergedDictionaries[0] = dict;
        IsDark = dark;
        Changed?.Invoke();
    }

    /// <summary>Re-applies the theme after Windows changed its light/dark mode or accent color.</summary>
    public static void OnSystemSettingsChanged()
    {
        var s = AppSettings.Current;
        if (s.Theme == "system" || s.UseSystemAccent) Apply();
    }

    public static bool TryGetSystemAccent(bool dark, out Color accent, out Color hover)
    {
        accent = hover = default;
        try
        {
            var ui = new Windows.UI.ViewManagement.UISettings();
            var a = ui.GetColorValue(dark ? Windows.UI.ViewManagement.UIColorType.AccentLight2 : Windows.UI.ViewManagement.UIColorType.AccentDark1);
            var h = ui.GetColorValue(dark ? Windows.UI.ViewManagement.UIColorType.AccentLight3 : Windows.UI.ViewManagement.UIColorType.Accent);
            accent = Color.FromRgb(a.R, a.G, a.B);
            hover = Color.FromRgb(h.R, h.G, h.B);
            return true;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>Accent color as Windows shows it in Settings (for the preview swatch).</summary>
    public static Color? SystemAccentPreview
    {
        get
        {
            try
            {
                var c = new Windows.UI.ViewManagement.UISettings().GetColorValue(Windows.UI.ViewManagement.UIColorType.Accent);
                return Color.FromRgb(c.R, c.G, c.B);
            }
            catch
            {
                return null;
            }
        }
    }

    private static double Luminance(Color c) => (0.2126 * c.R + 0.7152 * c.G + 0.0722 * c.B) / 255.0;

    private static SolidColorBrush Brush(Color c)
    {
        var b = new SolidColorBrush(c);
        b.Freeze();
        return b;
    }
}
