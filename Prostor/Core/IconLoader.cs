using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace Prostor.Core;

/// <summary>Extracts icons from executables, .ico files, shortcuts and images. Results are frozen (thread-safe).</summary>
public static class IconLoader
{
    private static readonly string[] ImageExtensions = { ".png", ".jpg", ".jpeg", ".bmp", ".gif" };

    /// <summary>Parses "C:\path\app.exe,0" / "\"C:\path\app.exe\",-101" style specs.</summary>
    public static ImageSource FromIconSpec(string spec, int size = 48)
    {
        if (string.IsNullOrWhiteSpace(spec)) return null;
        string s = Environment.ExpandEnvironmentVariables(spec.Trim());
        int index = 0;
        string path;
        if (s.StartsWith('"'))
        {
            int end = s.IndexOf('"', 1);
            path = end > 0 ? s.Substring(1, end - 1) : s.Trim('"');
            string rest = end > 0 ? s.Substring(end + 1).Trim() : "";
            if (rest.StartsWith(',')) int.TryParse(rest.Substring(1).Trim(), out index);
        }
        else
        {
            path = s;
            int comma = s.LastIndexOf(',');
            if (comma > 0 && int.TryParse(s.Substring(comma + 1).Trim(), out int i))
            {
                index = i;
                path = s.Substring(0, comma);
            }
        }
        path = path.Trim().Trim('"');
        if (!File.Exists(path)) return null;
        return FromFile(path, size, index);
    }

    public static ImageSource FromFile(string path, int size = 48, int index = 0)
    {
        try
        {
            if (string.IsNullOrEmpty(path) || !File.Exists(path)) return null;
            string ext = Path.GetExtension(path).ToLowerInvariant();
            if (ImageExtensions.Contains(ext)) return FromImage(path, size);

            if (ext is ".exe" or ".dll" or ".ico" or ".icl" or ".ocx" or ".cpl")
            {
                if (index >= 0)
                {
                    var icons = new IntPtr[1];
                    var ids = new uint[1];
                    uint n = Native.PrivateExtractIcons(path, index, size, size, icons, ids, 1, 0);
                    if (n != 0 && n != uint.MaxValue && icons[0] != IntPtr.Zero)
                    {
                        var img = FromHIcon(icons[0]);
                        if (img != null) return img;
                    }
                }
                else
                {
                    var large = new IntPtr[1];
                    if (Native.ExtractIconEx(path, index, large, null, 1) > 0 && large[0] != IntPtr.Zero)
                    {
                        var img = FromHIcon(large[0]);
                        if (img != null) return img;
                    }
                }
            }
            return FromShell(path);
        }
        catch
        {
            return null;
        }
    }

    public static ImageSource FromShell(string path)
    {
        try
        {
            var info = new Native.SHFILEINFO();
            IntPtr r = Native.SHGetFileInfo(path, 0, ref info, (uint)Marshal.SizeOf<Native.SHFILEINFO>(),
                Native.SHGFI_ICON | Native.SHGFI_LARGEICON);
            if (r == IntPtr.Zero || info.hIcon == IntPtr.Zero) return null;
            return FromHIcon(info.hIcon);
        }
        catch
        {
            return null;
        }
    }

    public static ImageSource FromImage(string path, int size)
    {
        try
        {
            var bmp = new BitmapImage();
            bmp.BeginInit();
            bmp.CacheOption = BitmapCacheOption.OnLoad;
            bmp.CreateOptions = BitmapCreateOptions.IgnoreColorProfile;
            bmp.DecodePixelWidth = size;
            bmp.UriSource = new Uri(path, UriKind.Absolute);
            bmp.EndInit();
            bmp.Freeze();
            return bmp;
        }
        catch
        {
            return null;
        }
    }

    private static ImageSource FromHIcon(IntPtr hIcon)
    {
        try
        {
            var src = Imaging.CreateBitmapSourceFromHIcon(hIcon, Int32Rect.Empty, BitmapSizeOptions.FromEmptyOptions());
            var conv = new FormatConvertedBitmap(src, PixelFormats.Pbgra32, null, 0);
            int w = conv.PixelWidth, h = conv.PixelHeight, stride = w * 4;
            var pixels = new byte[stride * h];
            conv.CopyPixels(pixels, stride, 0);
            bool empty = true;
            for (int i = 3; i < pixels.Length; i += 4)
            {
                if (pixels[i] != 0) { empty = false; break; }
            }
            if (empty) return null;
            var bs = BitmapSource.Create(w, h, 96, 96, PixelFormats.Pbgra32, null, pixels, stride);
            bs.Freeze();
            return bs;
        }
        catch
        {
            return null;
        }
        finally
        {
            Native.DestroyIcon(hIcon);
        }
    }
}
