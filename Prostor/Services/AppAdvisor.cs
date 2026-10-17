using System.Text.RegularExpressions;
using Prostor.Models;

namespace Prostor.Services;

public enum AppHint { None, Dependency, Driver, System, Preinstalled }

/// <summary>Tells which programs are safe to remove and which other software depends on.</summary>
public static class AppAdvisor
{
    private const RegexOptions Opt = RegexOptions.IgnoreCase | RegexOptions.Compiled;

    private static readonly Regex DependencyName = new(
        @"^Microsoft Visual C\+\+ .*(Redistributable|Runtime)|Visual C\+\+ Library CRT|^Microsoft \.NET|\.NET (Core )?(Runtime|Desktop Runtime|Framework)|" +
        @"^Microsoft Windows Desktop Runtime|^Microsoft ASP\.NET Core|DirectX|^Java(\(TM\))?( \d+)?( Update \d+)?( \(64-bit\))?$|^Java \d+|" +
        @"Edge WebView2|^Vulkan Run ?Time|PhysX|^OpenAL$|XNA Framework|Microsoft GameInput|Visual Studio Tools for Applications|" +
        @"^Microsoft ODBC Driver|^Microsoft OLE DB|^Windows App Runtime|Windows App SDK|^Microsoft Update Health Tools|^Python Launcher$|" +
        @"^Microsoft Visual Studio Installer$|^Universal CRT|^Windows Software Development Kit|^Windows SDK AddOn|^vs_.*(msi|x64|x86)?$|" +
        @"^Microsoft Gaming Services$|^Gaming Services$|^Xbox Identity Provider$|^NVIDIA PhysX", Opt);

    private static readonly Regex DriverPublisher = new(
        @"NVIDIA|Realtek|Intel|Advanced Micro Devices|\bAMD\b|Synaptics|ELAN|Qualcomm|Broadcom|MediaTek|Killer|Dolby|Conexant|IDT|" +
        @"Micro-Star|ASUSTeK|GIGABYTE|ASRock|Logitech|Razer|Corsair|SteelSeries|HyperX|Wacom|Huion|Kingston|Samsung Electronics|Western Digital|Seagate", Opt);

    private static readonly Regex DriverName = new(
        @"driver|драйвер|chipset|graphics|audio|hd audio|sound|management engine|rapid storage|optane|bluetooth|wi-?fi|wireless|ethernet|" +
        @"\blan\b|serial io|thunderbolt|\busb\b|touchpad|gpio|firmware|control center|control panel|software component|" +
        @"display|monitor|nahimic|sonic studio|armoury|dragon center|msi center|g hub|icue|synapse", Opt);

    private static readonly Regex SystemName = new(
        @"^Microsoft Edge$|^Microsoft Edge Update$|^Microsoft Update Health Tools$|^Windows Subsystem for Linux Update$|" +
        @"^Microsoft Windows .*Update|^Update for |^Microsoft Policy Platform$|^Microsoft Intune Management Extension$", Opt);

    /// <summary>Store apps that come with Windows or the PC vendor and are rarely needed.</summary>
    private static readonly string[] PreinstalledPackages =
    {
        "king.com.", "Microsoft.BingNews", "Microsoft.BingWeather", "Microsoft.BingSearch", "Microsoft.BingFinance", "Microsoft.BingSports",
        "Microsoft.GetHelp", "Microsoft.Getstarted", "Microsoft.MicrosoftSolitaireCollection", "Microsoft.People", "Microsoft.WindowsFeedbackHub",
        "Microsoft.ZuneVideo", "Microsoft.MicrosoftOfficeHub", "Microsoft.SkypeApp", "Microsoft.549981C3F5F10", "Microsoft.WindowsMaps",
        "Microsoft.MixedReality.Portal", "Microsoft.Microsoft3DViewer", "Microsoft.MSPaint", "Microsoft.Todos", "Microsoft.PowerAutomateDesktop",
        "MicrosoftTeams", "Clipchamp.Clipchamp", "Disney.", "SpotifyAB.SpotifyMusic", "BytedancePte.Ltd.TikTok", "Facebook.", "AmazonVideo.PrimeVideo",
        "5A894077.McAfeeSecurity", "7EE7776C.LinkedInforWindows", "Microsoft.MicrosoftJournal", "Microsoft.Wallet", "Microsoft.OneConnect",
        "Microsoft.Messaging", "Microsoft.Print3D", "Microsoft.XboxApp", "Microsoft.OutlookForWindows", "Microsoft.WindowsCommunicationsApps",
        "Microsoft.MicrosoftStickyNotes", "Microsoft.YourPhone", "Microsoft.Windows.DevHome", "Microsoft.MicrosoftFamily", "MicrosoftCorporationII.MicrosoftFamily",
        "MicrosoftCorporationII.QuickAssist", "Microsoft.WindowsSoundRecorder", "Microsoft.WindowsAlarms", "Microsoft.ZuneMusic", "AD2F1837.",
        "E046963F.", "DellInc.", "AcerIncorporated.", "B9ECED6F.", "4DF9E0F8.Netflix", "ActiproSoftwareLLC.", "Duolingo-LearnLanguagesforFree",
    };

    private static readonly Regex PreinstalledDesktop = new(@"McAfee|WildTangent|Norton (Security|360)|Booking\.com|Dropbox (Promotion|Offer)|ExpressVPN Offer|CyberLink .*Trial", Opt);

    public static AppHint Classify(InstalledApp app)
    {
        string name = app.Name ?? "";
        if (app.IsStore)
        {
            string pfn = app.PackageFamilyName ?? "";
            if (pfn.StartsWith("Microsoft.GamingServices", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.XboxIdentityProvider", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.HEIFImageExtension", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.HEVCVideoExtension", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.VP9VideoExtensions", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.WebMediaExtensions", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.WebpImageExtension", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.AV1VideoExtension", StringComparison.OrdinalIgnoreCase) ||
                pfn.StartsWith("Microsoft.RawImageExtension", StringComparison.OrdinalIgnoreCase))
                return AppHint.Dependency;
            if (PreinstalledPackages.Any(p => pfn.StartsWith(p, StringComparison.OrdinalIgnoreCase))) return AppHint.Preinstalled;
            return AppHint.None;
        }
        if (app.IsGame) return AppHint.None;
        if (SystemName.IsMatch(name)) return AppHint.System;
        if (DependencyName.IsMatch(name)) return AppHint.Dependency;
        if (PreinstalledDesktop.IsMatch(name)) return AppHint.Preinstalled;
        string publisher = app.Publisher ?? "";
        if ((DriverPublisher.IsMatch(publisher) || DriverPublisher.IsMatch(name)) && DriverName.IsMatch(name)) return AppHint.Driver;
        if (Regex.IsMatch(name, @"\bdriver\b|драйвер", RegexOptions.IgnoreCase)) return AppHint.Driver;
        return AppHint.None;
    }

    public static string Label(AppHint hint) => hint switch
    {
        AppHint.Dependency => "Нужна другим программам",
        AppHint.Driver => "Драйвер",
        AppHint.System => "Часть Windows",
        AppHint.Preinstalled => "Предустановлено",
        _ => null
    };

    public static string Explanation(AppHint hint) => hint switch
    {
        AppHint.Dependency => "Это библиотека или среда выполнения, которой пользуются другие программы и игры. Без неё они могут перестать запускаться — удаляйте, только если точно знаете, что она не нужна.",
        AppHint.Driver => "Драйвер или утилита устройства. После удаления может пропасть звук, сеть, подсветка или настройки видеокарты.",
        AppHint.System => "Компонент, который Windows использует сама. Удалять не рекомендуется — система может восстановить его или работать с ошибками.",
        AppHint.Preinstalled => "Пришло вместе с Windows или компьютером. Если вы этим не пользуетесь — можно смело удалить.",
        _ => null
    };

    /// <summary>"warning" / "danger" / "success" — color of the label.</summary>
    public static string Tone(AppHint hint) => hint switch
    {
        AppHint.Dependency or AppHint.Driver => "warning",
        AppHint.System => "danger",
        AppHint.Preinstalled => "success",
        _ => null
    };
}
