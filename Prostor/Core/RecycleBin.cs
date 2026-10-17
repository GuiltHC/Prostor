using System.Security.Principal;
using System.Text;

namespace Prostor.Core;

/// <summary>Puts items back from the Recycle Bin by their original path (reads the $I metadata files directly).</summary>
public static class RecycleBin
{
    public static bool Restore(string originalPath)
    {
        try
        {
            string full = Path.GetFullPath(originalPath).TrimEnd('\\');
            if (File.Exists(full) || Directory.Exists(full)) return false; // something else is there now
            string root = Path.GetPathRoot(full);
            string sid = WindowsIdentity.GetCurrent().User?.Value;
            if (root == null || sid == null) return false;
            string bin = Path.Combine(root, "$Recycle.Bin", sid);
            if (!Directory.Exists(bin)) return false;

            string bestInfo = null;
            long bestTime = long.MinValue;
            foreach (var info in Directory.EnumerateFiles(bin, "$I*"))
            {
                if (!TryReadInfo(info, out string path, out long deleted)) continue;
                if (!string.Equals(path.TrimEnd('\\'), full, StringComparison.OrdinalIgnoreCase) || deleted <= bestTime) continue;
                bestInfo = info;
                bestTime = deleted;
            }
            if (bestInfo == null) return false;

            string data = Path.Combine(bin, "$R" + Path.GetFileName(bestInfo).Substring(2));
            string parent = Path.GetDirectoryName(full);
            if (!string.IsNullOrEmpty(parent)) Directory.CreateDirectory(parent);
            if (Directory.Exists(data)) Directory.Move(data, full);
            else if (File.Exists(data)) File.Move(data, full);
            else return false;
            File.Delete(bestInfo);
            return true;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>Parses a $I file (Windows Vista+: version 1, Windows 10+: version 2).</summary>
    public static bool TryReadInfo(string file, out string originalPath, out long deletedFileTime)
    {
        originalPath = null;
        deletedFileTime = 0;
        try
        {
            var b = File.ReadAllBytes(file);
            if (b.Length < 24) return false;
            long version = BitConverter.ToInt64(b, 0);
            deletedFileTime = BitConverter.ToInt64(b, 16);
            if (version == 2 && b.Length >= 28)
            {
                int chars = BitConverter.ToInt32(b, 24);
                if (chars <= 0 || 28 + chars * 2 > b.Length) return false;
                originalPath = Encoding.Unicode.GetString(b, 28, chars * 2).TrimEnd('\0');
            }
            else if (version == 1 && b.Length >= 24 + 520)
            {
                originalPath = Encoding.Unicode.GetString(b, 24, 520).TrimEnd('\0');
            }
            return !string.IsNullOrEmpty(originalPath);
        }
        catch
        {
            return false;
        }
    }
}
