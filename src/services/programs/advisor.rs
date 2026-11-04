//! Which programs are safe to remove and which other software depends on (port of Services/AppAdvisor.cs).

use super::inventory::App;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hint {
    #[default]
    None,
    Dependency,
    Driver,
    System,
    Preinstalled,
}

fn rx(p: &str) -> Regex {
    Regex::new(&format!("(?i){p}")).expect("valid regex")
}

static DEPENDENCY_NAME: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"^Microsoft Visual C\+\+ .*(Redistributable|Runtime)|Visual C\+\+ Library CRT|^Microsoft \.NET|\.NET (Core )?(Runtime|Desktop Runtime|Framework)|^Microsoft Windows Desktop Runtime|^Microsoft ASP\.NET Core|DirectX|^Java(\(TM\))?( \d+)?( Update \d+)?( \(64-bit\))?$|^Java \d+|Edge WebView2|^Vulkan Run ?Time|PhysX|^OpenAL$|XNA Framework|Microsoft GameInput|Visual Studio Tools for Applications|^Microsoft ODBC Driver|^Microsoft OLE DB|^Windows App Runtime|Windows App SDK|^Microsoft Update Health Tools|^Python Launcher$|^Microsoft Visual Studio Installer$|^Universal CRT|^Windows Software Development Kit|^Windows SDK AddOn|^vs_.*(msi|x64|x86)?$|^Microsoft Gaming Services$|^Gaming Services$|^Xbox Identity Provider$|^NVIDIA PhysX")
});
static DRIVER_PUBLISHER: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"NVIDIA|Realtek|Intel|Advanced Micro Devices|\bAMD\b|Synaptics|ELAN|Qualcomm|Broadcom|MediaTek|Killer|Dolby|Conexant|IDT|Micro-Star|ASUSTeK|GIGABYTE|ASRock|Logitech|Razer|Corsair|SteelSeries|HyperX|Wacom|Huion|Kingston|Samsung Electronics|Western Digital|Seagate")
});
static DRIVER_NAME: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"driver|драйвер|chipset|graphics|audio|hd audio|sound|management engine|rapid storage|optane|bluetooth|wi-?fi|wireless|ethernet|\blan\b|serial io|thunderbolt|\busb\b|touchpad|gpio|firmware|control center|control panel|software component|display|monitor|nahimic|sonic studio|armoury|dragon center|msi center|g hub|icue|synapse")
});
static SYSTEM_NAME: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"^Microsoft Edge$|^Microsoft Edge Update$|^Microsoft Update Health Tools$|^Windows Subsystem for Linux Update$|^Microsoft Windows .*Update|^Update for |^Microsoft Policy Platform$|^Microsoft Intune Management Extension$")
});
static PREINSTALLED_DESKTOP: LazyLock<Regex> =
    LazyLock::new(|| rx(r"McAfee|WildTangent|Norton (Security|360)|Booking\.com|Dropbox (Promotion|Offer)|ExpressVPN Offer|CyberLink .*Trial"));
static PLAIN_DRIVER: LazyLock<Regex> = LazyLock::new(|| rx(r"\bdriver\b|драйвер"));

const STORE_DEPENDENCIES: &[&str] = &[
    "Microsoft.GamingServices",
    "Microsoft.XboxIdentityProvider",
    "Microsoft.HEIFImageExtension",
    "Microsoft.HEVCVideoExtension",
    "Microsoft.VP9VideoExtensions",
    "Microsoft.WebMediaExtensions",
    "Microsoft.WebpImageExtension",
    "Microsoft.AV1VideoExtension",
    "Microsoft.RawImageExtension",
];

/// Store apps that come with Windows or the PC vendor and are rarely needed.
const PREINSTALLED_PACKAGES: &[&str] = &[
    "king.com.",
    "Microsoft.BingNews",
    "Microsoft.BingWeather",
    "Microsoft.BingSearch",
    "Microsoft.BingFinance",
    "Microsoft.BingSports",
    "Microsoft.GetHelp",
    "Microsoft.Getstarted",
    "Microsoft.MicrosoftSolitaireCollection",
    "Microsoft.People",
    "Microsoft.WindowsFeedbackHub",
    "Microsoft.ZuneVideo",
    "Microsoft.MicrosoftOfficeHub",
    "Microsoft.SkypeApp",
    "Microsoft.549981C3F5F10",
    "Microsoft.WindowsMaps",
    "Microsoft.MixedReality.Portal",
    "Microsoft.Microsoft3DViewer",
    "Microsoft.MSPaint",
    "Microsoft.Todos",
    "Microsoft.PowerAutomateDesktop",
    "MicrosoftTeams",
    "Clipchamp.Clipchamp",
    "Disney.",
    "SpotifyAB.SpotifyMusic",
    "BytedancePte.Ltd.TikTok",
    "Facebook.",
    "AmazonVideo.PrimeVideo",
    "5A894077.McAfeeSecurity",
    "7EE7776C.LinkedInforWindows",
    "Microsoft.MicrosoftJournal",
    "Microsoft.Wallet",
    "Microsoft.OneConnect",
    "Microsoft.Messaging",
    "Microsoft.Print3D",
    "Microsoft.XboxApp",
    "Microsoft.OutlookForWindows",
    "Microsoft.WindowsCommunicationsApps",
    "Microsoft.MicrosoftStickyNotes",
    "Microsoft.YourPhone",
    "Microsoft.Windows.DevHome",
    "Microsoft.MicrosoftFamily",
    "MicrosoftCorporationII.MicrosoftFamily",
    "MicrosoftCorporationII.QuickAssist",
    "Microsoft.WindowsSoundRecorder",
    "Microsoft.WindowsAlarms",
    "Microsoft.ZuneMusic",
    "AD2F1837.",
    "E046963F.",
    "DellInc.",
    "AcerIncorporated.",
    "B9ECED6F.",
    "4DF9E0F8.Netflix",
    "ActiproSoftwareLLC.",
    "Duolingo-LearnLanguagesforFree",
];

fn starts_ci(s: &str, p: &str) -> bool {
    s.to_lowercase().starts_with(&p.to_lowercase())
}

pub fn classify(app: &App) -> Hint {
    let name = app.name.as_str();
    if app.is_store {
        let pfn = app.package_family_name.as_deref().unwrap_or_default();
        if STORE_DEPENDENCIES.iter().any(|p| starts_ci(pfn, p)) {
            return Hint::Dependency;
        }
        if PREINSTALLED_PACKAGES.iter().any(|p| starts_ci(pfn, p)) {
            return Hint::Preinstalled;
        }
        return Hint::None;
    }
    if app.is_game() {
        return Hint::None;
    }
    if SYSTEM_NAME.is_match(name) {
        return Hint::System;
    }
    if DEPENDENCY_NAME.is_match(name) {
        return Hint::Dependency;
    }
    if PREINSTALLED_DESKTOP.is_match(name) {
        return Hint::Preinstalled;
    }
    let publisher = app.publisher.as_deref().unwrap_or_default();
    if (DRIVER_PUBLISHER.is_match(publisher) || DRIVER_PUBLISHER.is_match(name)) && DRIVER_NAME.is_match(name) {
        return Hint::Driver;
    }
    if PLAIN_DRIVER.is_match(name) {
        return Hint::Driver;
    }
    Hint::None
}

impl Hint {
    pub fn label(self) -> Option<&'static str> {
        Some(match self {
            Hint::Dependency => "Нужна другим программам",
            Hint::Driver => "Драйвер",
            Hint::System => "Часть Windows",
            Hint::Preinstalled => "Предустановлено",
            Hint::None => return None,
        })
    }

    pub fn explanation(self) -> Option<&'static str> {
        Some(match self {
            Hint::Dependency => {
                "Это библиотека или среда выполнения, которой пользуются другие программы и игры. Без неё они могут перестать запускаться — удаляйте, только если точно знаете, что она не нужна."
            }
            Hint::Driver => "Драйвер или утилита устройства. После удаления может пропасть звук, сеть, подсветка или настройки видеокарты.",
            Hint::System => "Компонент, который Windows использует сама. Удалять не рекомендуется — система может восстановить его или работать с ошибками.",
            Hint::Preinstalled => "Пришло вместе с Windows или компьютером. Если вы этим не пользуетесь — можно смело удалить.",
            Hint::None => return None,
        })
    }

    pub fn risky(self) -> bool {
        matches!(self, Hint::Dependency | Hint::Driver | Hint::System)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, publisher: &str) -> App {
        App { name: name.into(), publisher: Some(publisher.into()), ..Default::default() }
    }

    #[test]
    fn classifies() {
        assert_eq!(classify(&app("Microsoft Visual C++ 2015-2022 Redistributable (x64)", "Microsoft")), Hint::Dependency);
        assert_eq!(classify(&app("NVIDIA Graphics Driver 560.94", "NVIDIA Corporation")), Hint::Driver);
        assert_eq!(classify(&app("Realtek High Definition Audio Driver", "Realtek")), Hint::Driver);
        assert_eq!(classify(&app("Microsoft Edge", "Microsoft")), Hint::System);
        assert_eq!(classify(&app("McAfee LiveSafe", "McAfee")), Hint::Preinstalled);
        assert_eq!(classify(&app("Telegram Desktop", "Telegram FZ-LLC")), Hint::None);
        let store = App { is_store: true, package_family_name: Some("Microsoft.BingNews_8wekyb3d8bbwe".into()), ..Default::default() };
        assert_eq!(classify(&store), Hint::Preinstalled);
    }
}
