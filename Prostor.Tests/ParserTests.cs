using Prostor.Core;
using Prostor.Services;
using Xunit;

namespace Prostor.Tests;

public class ParserTests
{
    [Fact]
    public void Vdf_ParsesNestedObjectsCommentsAndEscapes()
    {
        const string text = """
            "libraryfolders"
            {
                // comment
                "0"
                {
                    "path"      "D:\\SteamLibrary"
                    "apps" { "570" "1234" "1361210" "99" }
                }
                "contentstatsid" "-123"
            }
            """;
        var root = SteamLibrary.Parse(text);
        var folders = Assert.IsType<Dictionary<string, object>>(root["libraryfolders"]);
        var lib = Assert.IsType<Dictionary<string, object>>(folders["0"]);
        Assert.Equal(@"D:\SteamLibrary", lib["path"]);
        var apps = Assert.IsType<Dictionary<string, object>>(lib["apps"]);
        Assert.Equal("1234", apps["570"]);
        Assert.Equal("-123", folders["ContentStatsId"]); // keys are case-insensitive
    }

    [Fact]
    public void CommandLine_SplitsQuotedAndUnquotedPaths()
    {
        string cmd = Path.Combine(Environment.SystemDirectory, "cmd.exe");
        var (file, args) = CommandLine.Split($"\"{cmd}\" /c echo 1");
        Assert.Equal(cmd, file, ignoreCase: true);
        Assert.Equal("/c echo 1", args);

        Assert.Equal(cmd, CommandLine.ExtractExecutable($"{cmd} /c echo"), ignoreCase: true);
        Assert.Null(CommandLine.ExtractExecutable(@"C:\NoSuchFolder\nothing.exe /S"));
    }

    [Fact]
    public void Winget_TableIsParsedByHeaderPositions()
    {
        const string output = "   - \r   \\ \r" +
            "Имя                                    ИД                                     Версия                        Доступно                      Источник\n" +
            "--------------------------------------------------------------------------------------------------------------------------------------------------\n" +
            "Audacity 3.7.7                         Audacity.Audacity                      3.7.7                         4.0.1.262721353               winget\n" +
            "Microsoft Visual Studio Code (User)    Microsoft.VisualStudioCode             1.129.1                       1.142.0                       winget\n" +
            "Python 3.14.5rc1 (64-bit)              Python.Python.3.14                     < 3.14.5                      3.14.7                        winget\n" +
            "Очень длинное название программы, к…   Vendor.LongName                        1.0                           2.0                           winget\n" +
            "Доступны обновления: 4.\n";
        var list = WingetService.ParseTable(output);
        Assert.Equal(4, list.Count);
        Assert.Equal("Audacity.Audacity", list[0].Id);
        Assert.Equal("4.0.1.262721353", list[0].Available);
        Assert.Equal("Microsoft Visual Studio Code (User)", list[1].Name);
        Assert.Equal("< 3.14.5", list[2].Version);
        Assert.Equal("winget", list[3].Source);
    }

    [Fact]
    public void Pnputil_ParsesLocalizedOutputAndFindsOutdatedPackages()
    {
        const string output = """
            Служебная программа PnP

            Опубликованное имя:     oem10.inf
            Исходное имя:      nv_dispi.inf
            Имя поставщика:      NVIDIA
            Имя класса:         Display
            Версия драйвера:     05/01/2025 32.0.15.7680

            Опубликованное имя:     oem22.inf
            Исходное имя:      nv_dispi.inf
            Имя поставщика:      NVIDIA
            Версия драйвера:     08/14/2026 32.0.15.8180

            Опубликованное имя:     oem5.inf
            Исходное имя:      rt640x64.inf
            Версия драйвера:     01/01/2024 10.68.0.0
            """;
        var packages = DeepCleanup.ParsePnputil(output);
        Assert.Equal(3, packages.Count);
        var outdated = DeepCleanup.OutdatedPackages(packages);
        var old = Assert.Single(outdated);
        Assert.Equal("oem10.inf", old.Published);
    }

    [Fact]
    public void BootEvents_AreReadFromEventXml()
    {
        const string xml = "<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><TimeCreated SystemTime='2025-08-04T16:32:45.9135675Z'/></System>" +
                           "<EventData><Data Name='BootTime'>39408</Data><Data Name='MainPathBootTime'>12608</Data><Data Name='BootNumStartupApps'>34</Data></EventData></Event>";
        var boots = StartupService.ParseBootEvents(xml);
        var b = Assert.Single(boots);
        Assert.Equal(12.608, b.MainPathSeconds, 3);
        Assert.Equal(39.408, b.TotalSeconds, 3);
        Assert.Equal(34, b.StartupApps);
    }

    [Fact]
    public void EpicManifest_SkipsDlcAndIncompleteInstalls()
    {
        var game = GameLaunchers.ParseEpicManifest(
            """{"AppName":"Fortnite","MainGameAppName":"Fortnite","DisplayName":"Fortnite","InstallLocation":"D:\\Games\\Fortnite","InstallSize":123456}""", "x.item");
        Assert.NotNull(game);
        Assert.Equal(123456, game.InstallSize);
        Assert.Equal(@"D:\Games\Fortnite", game.InstallLocation);

        Assert.Null(GameLaunchers.ParseEpicManifest("""{"AppName":"DlcPack","MainGameAppName":"Fortnite"}""", "y.item"));
        Assert.Null(GameLaunchers.ParseEpicManifest("""{"AppName":"Half","MainGameAppName":"Half","bIsIncompleteInstall":true}""", "z.item"));
    }

    [Fact]
    public void UserAssist_NamesAreRot13()
    {
        Assert.Equal("Valve.Steam.Client", UsageTracker.Rot13("Inyir.Fgrnz.Pyvrag"));
        Assert.Equal("C:\\Ab", UsageTracker.Rot13(UsageTracker.Rot13("C:\\Ab")));
    }
}
