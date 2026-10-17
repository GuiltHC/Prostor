using System.Globalization;

namespace Prostor.Core;

/// <summary>Russian-friendly formatting of sizes, counts and dates.</summary>
public static class Fmt
{
    public static readonly CultureInfo Ru = CultureInfo.GetCultureInfo("ru-RU");
    private static readonly string[] Units = { "КБ", "МБ", "ГБ", "ТБ", "ПБ" };

    public static string Size(long bytes)
    {
        if (bytes < 0) return "—";
        if (bytes < 1024) return bytes.ToString(Ru) + " Б";
        double v = bytes / 1024.0;
        int i = 0;
        while (v >= 1024 && i < Units.Length - 1)
        {
            v /= 1024;
            i++;
        }
        string num = v >= 100 ? v.ToString("0", Ru) : v.ToString("0.#", Ru);
        return num + " " + Units[i];
    }

    public static string Number(long n) => n.ToString("N0", Ru);

    public static string Plural(long n, string one, string few, string many)
    {
        n = Math.Abs(n);
        long mod100 = n % 100;
        if (mod100 >= 11 && mod100 <= 14) return many;
        return (n % 10) switch
        {
            1 => one,
            2 or 3 or 4 => few,
            _ => many
        };
    }

    public static string Count(long n, string one, string few, string many) =>
        Number(n) + " " + Plural(n, one, few, many);

    public static string Files(long n) => Count(n, "файл", "файла", "файлов");
    public static string Folders(long n) => Count(n, "папка", "папки", "папок");
    public static string Programs(long n) => Count(n, "программа", "программы", "программ");

    public static string Date(DateTime? d) => d.HasValue ? d.Value.ToString("d MMM yyyy", Ru).Replace(".", "") : "—";

    public static string Ago(DateTime? date)
    {
        if (!date.HasValue) return "";
        var days = (DateTime.Now.Date - date.Value.Date).TotalDays;
        if (days < 1) return "сегодня";
        if (days < 2) return "вчера";
        if (days < 30)
        {
            int n = (int)days;
            return $"{n} {Plural(n, "день", "дня", "дней")} назад";
        }
        if (days < 365)
        {
            int n = Math.Max(1, (int)(days / 30.4));
            return $"{n} {Plural(n, "месяц", "месяца", "месяцев")} назад";
        }
        int y = Math.Max(1, (int)(days / 365.25));
        return $"{y} {Plural(y, "год", "года", "лет")} назад";
    }

    public static string Percent(double fraction)
    {
        double p = fraction * 100;
        if (p <= 0) return "0%";
        if (p < 0.1) return "<0,1%";
        return p.ToString(p >= 10 ? "0" : "0.#", Ru) + "%";
    }

    /// <summary>"0:42", "12:05", "1:02:03".</summary>
    public static string Clock(TimeSpan t)
    {
        if (t < TimeSpan.Zero) t = TimeSpan.Zero;
        return t.TotalHours >= 1 ? $"{(int)t.TotalHours}:{t.Minutes:00}:{t.Seconds:00}" : $"{(int)t.TotalMinutes}:{t.Seconds:00}";
    }

    public static string Duration(TimeSpan t)
    {
        if (t.TotalSeconds < 60) return $"{Math.Max(0, (int)t.TotalSeconds)} с";
        if (t.TotalMinutes < 60) return $"{(int)t.TotalMinutes} мин {t.Seconds} с";
        return $"{(int)t.TotalHours} ч {t.Minutes} мин";
    }
}
