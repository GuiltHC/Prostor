using System.Text;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;
using Prostor.ViewModels;
using Xunit;

namespace Prostor.Tests;

public class LogicTests
{
    [Fact]
    public void Fmt_FormatsSizesPluralsAndClock()
    {
        Assert.Equal("512 Б", Fmt.Size(512));
        Assert.Equal("1,5 КБ", Fmt.Size(1536));
        Assert.Equal("120 ГБ", Fmt.Size(120L << 30));
        Assert.Equal("1 файл", Fmt.Files(1));
        Assert.Equal("3 файла", Fmt.Files(3));
        Assert.Equal("11 файлов", Fmt.Files(11));
        Assert.Equal("21 программа", Fmt.Programs(21));
        Assert.Equal("1:05", Fmt.Clock(TimeSpan.FromSeconds(65)));
        Assert.Equal("1:02:03", Fmt.Clock(new TimeSpan(1, 2, 3)));
    }

    [Fact]
    public void Leftovers_NamesLoseVersionsArchitectureAndCompanySuffix()
    {
        Assert.Contains("Foo Bar", LeftoverFinder.CleanNames("Foo Bar 2.3.1 (x64)"));
        Assert.Contains("FooBar", LeftoverFinder.CleanNames("Foo Bar 2.3.1 (x64)"));
        Assert.Equal("Contoso", LeftoverFinder.CleanPublisher("Contoso, Inc."));
        Assert.Empty(LeftoverFinder.CleanNames("ab"));
    }

    [Fact]
    public void Leftovers_NeverTouchSystemFolders()
    {
        Assert.False(AppInventory.IsUsableLocationPath(@"C:\"));
        Assert.False(AppInventory.IsUsableLocationPath(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles)));
        Assert.False(AppInventory.IsUsableLocationPath(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Windows), "System32")));
        Assert.False(AppInventory.IsUsableLocationPath(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData)));
        Assert.True(AppInventory.IsUsableLocationPath(Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Some App")));
    }

    [Fact]
    public void Exclusions_CoverSubfoldersButNotSiblings()
    {
        var s = new AppSettings { Exclusions = { @"D:\Photos" } };
        Assert.True(s.IsExcluded(@"D:\Photos"));
        Assert.True(s.IsExcluded(@"d:\photos\2024\img.jpg"));
        Assert.False(s.IsExcluded(@"D:\Photos2"));
        Assert.False(s.IsExcluded(@"D:\"));
    }

    [Fact]
    public void Advisor_LabelsRuntimesDriversAndPreinstalledApps()
    {
        Assert.Equal(AppHint.Dependency, AppAdvisor.Classify(new InstalledApp { Name = "Microsoft Visual C++ 2015-2022 Redistributable (x64) - 14.40.33810" }));
        Assert.Equal(AppHint.Driver, AppAdvisor.Classify(new InstalledApp { Name = "NVIDIA Graphics Driver 560.94", Publisher = "NVIDIA Corporation" }));
        Assert.Equal(AppHint.Driver, AppAdvisor.Classify(new InstalledApp { Name = "Realtek High Definition Audio Driver", Publisher = "Realtek Semiconductor Corp." }));
        Assert.Equal(AppHint.Preinstalled, AppAdvisor.Classify(new InstalledApp { Name = "Candy Crush Saga", IsStore = true, PackageFamilyName = "king.com.CandyCrushSaga_kgqvnymyfvs32" }));
        Assert.Equal(AppHint.None, AppAdvisor.Classify(new InstalledApp { Name = "Telegram Desktop", Publisher = "Telegram FZ-LLC" }));
    }

    [Fact]
    public void Winget_UpgradesAreMatchedToProgramsByName()
    {
        var apps = new List<InstalledApp>
        {
            new() { Name = "OBS Studio", Version = "32.1.2" },
            new() { Name = "Очень длинное название программы, которое обрезано" },
            new() { Name = "Notepad" },
        };
        WingetService.Apply(apps, new[]
        {
            new WingetUpgrade("OBS Studio", "OBSProject.OBSStudio", "32.1.2", "32.2.2", "winget"),
            new WingetUpgrade("Очень длинное название программы, к…", "Vendor.Long", "1.0", "2.0", "winget"),
        });
        Assert.Equal("32.2.2", apps[0].UpdateVersion);
        Assert.Equal("OBSProject.OBSStudio", apps[0].WingetId);
        Assert.Equal("2.0", apps[1].UpdateVersion);
        Assert.False(apps[2].HasUpdate);
    }

    [Fact]
    public void Csv_QuotesSeparatorsAndQuotes()
    {
        var csv = ProgramsViewModel.BuildCsv(new[] { new InstalledApp { Name = "A; \"B\"", Publisher = "P", Version = "1" } });
        Assert.Contains("\"A; \"\"B\"\"\";P;1;", csv);
    }

    [Fact]
    public void Forecast_PredictsWhenTheDiskFillsUp()
    {
        var start = DateTime.Today.AddDays(-20);
        var points = Enumerable.Range(0, 21).Select(i => new DiskPoint
        {
            Date = start.AddDays(i), Free = (100L << 30) - i * (1L << 30), Total = 500L << 30
        }).ToList();
        var f = DiskHistory.Forecast(points);
        Assert.NotNull(f);
        Assert.Equal(-(1L << 30), f.BytesPerDay, 1e6);
        Assert.Equal(80, f.DaysUntilFull);

        Assert.Null(DiskHistory.Forecast(points.Take(2).ToList()));
    }

    [Fact]
    public void ScanComparison_ReportsTheDeepestFolderThatGrew()
    {
        var before = new ScanSnapshot { Root = "C:", Time = DateTime.Now.AddDays(-7), Total = 100L << 30 };
        before.Dirs["Users"] = 50L << 30;
        before.Dirs[@"Users\me"] = 49L << 30;
        before.Dirs[@"Users\me\Downloads"] = 2L << 30;
        before.Dirs["Windows"] = 30L << 30;
        var after = new ScanSnapshot { Root = "C:", Time = DateTime.Now, Total = 110L << 30 };
        after.Dirs["Users"] = 60L << 30;
        after.Dirs[@"Users\me"] = 59L << 30;
        after.Dirs[@"Users\me\Downloads"] = 12L << 30;
        after.Dirs["Windows"] = 30L << 30;

        var c = ScanHistory.Compare(before, after);
        var change = Assert.Single(c.Changes);
        Assert.Equal(@"Users\me\Downloads", change.RelativePath);
        Assert.Equal(10L << 30, change.Delta);
        Assert.Equal(10L << 30, c.Delta);
    }

    [Fact]
    public void Mft_DataRunsAreDecodedWithSignedOffsets()
    {
        // 0x31: 1-byte length, 3-byte offset → 0x10 clusters at LCN 0x0C0000; then 0x11: length 8 at offset -16
        byte[] runs = { 0x31, 0x10, 0x00, 0x00, 0x0C, 0x11, 0x08, 0xF0, 0x00 };
        var list = MftScanner.ParseRuns(runs);
        Assert.Equal(2, list.Count);
        Assert.Equal((0xC0000L, 0x10L), list[0]);
        Assert.Equal((0xC0000L - 16, 8L), list[1]);
    }

    [Fact]
    public void Mft_FixupsRestoreSectorEnds()
    {
        var rec = new byte[1024];
        Encoding.ASCII.GetBytes("FILE").CopyTo(rec, 0);
        BitConverter.GetBytes((ushort)0x30).CopyTo(rec, 4); // USA offset
        BitConverter.GetBytes((ushort)3).CopyTo(rec, 6);    // USN + 2 sectors
        BitConverter.GetBytes((ushort)0xABCD).CopyTo(rec, 0x30);
        BitConverter.GetBytes((ushort)0x1111).CopyTo(rec, 0x32);
        BitConverter.GetBytes((ushort)0x2222).CopyTo(rec, 0x34);
        BitConverter.GetBytes((ushort)0xABCD).CopyTo(rec, 510);
        BitConverter.GetBytes((ushort)0xABCD).CopyTo(rec, 1022);
        Assert.True(MftScanner.ApplyFixups(rec));
        Assert.Equal(0x1111, BitConverter.ToUInt16(rec, 510));
        Assert.Equal(0x2222, BitConverter.ToUInt16(rec, 1022));

        BitConverter.GetBytes((ushort)0x9999).CopyTo(rec, 510); // torn write
        rec[0x30] = 0x99;
        rec[0x31] = 0x99;
        BitConverter.GetBytes((ushort)0x1234).CopyTo(rec, 1022);
        Assert.False(MftScanner.ApplyFixups(rec));
    }

    [Fact]
    public void RecycleBin_ReadsVersion2InfoFiles()
    {
        string path = @"D:\Folder\file.txt";
        var data = new List<byte>();
        data.AddRange(BitConverter.GetBytes(2L));
        data.AddRange(BitConverter.GetBytes(1234L));
        data.AddRange(BitConverter.GetBytes(DateTime.Now.ToFileTime()));
        data.AddRange(BitConverter.GetBytes(path.Length + 1));
        data.AddRange(Encoding.Unicode.GetBytes(path + "\0"));
        string file = Path.Combine(Path.GetTempPath(), "$Iprostor-test.txt");
        File.WriteAllBytes(file, data.ToArray());
        try
        {
            Assert.True(RecycleBin.TryReadInfo(file, out string original, out long deleted));
            Assert.Equal(path, original);
            Assert.True(deleted > 0);
        }
        finally
        {
            File.Delete(file);
        }
    }

    [Fact]
    public void Duplicates_AreFoundByContentAndHardLinksIgnored()
    {
        string dir = Path.Combine(Path.GetTempPath(), "prostor-dup-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        try
        {
            var bytes = new byte[300_000];
            new Random(1).NextBytes(bytes);
            File.WriteAllBytes(Path.Combine(dir, "a.bin"), bytes);
            File.WriteAllBytes(Path.Combine(dir, "b.bin"), bytes);
            bytes[200_000] ^= 1;
            File.WriteAllBytes(Path.Combine(dir, "c.bin"), bytes); // same size, different content

            var root = new DiskScanner().Scan(dir, TestContext.Current.CancellationToken);
            var groups = new DuplicateFinder().Find(root, 1000, _ => false, TestContext.Current.CancellationToken);
            var g = Assert.Single(groups);
            Assert.Equal(2, g.Files.Count);
            Assert.DoesNotContain(g.Files, f => f.Name == "c.bin");
            Assert.Equal(300_000, g.Wasted);
        }
        finally
        {
            Directory.Delete(dir, true);
        }
    }
}
