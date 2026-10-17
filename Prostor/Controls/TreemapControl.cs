using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Controls;

public sealed class TreemapHit
{
    public DirNode Dir { get; init; }
    public DirNode FileParent { get; init; }
    public string FileName { get; init; }
    public long Size { get; init; }
    public byte Kind { get; init; }
    public byte Category { get; init; }

    public bool IsFile => FileName != null;
    public bool IsRest => Kind == TreemapControl.KindRest;
    public DirNode OwnerDir => Dir ?? FileParent;

    public string Title => IsRest ? "Мелкие файлы и папки" : IsFile ? FileName : Dir?.IsVirtual == true ? Dir.Name : Dir?.Name;
    public string FullPath => IsFile ? Path.Combine(FileParent.FullPath, FileName) : IsRest ? FileParent?.FullPath : Dir?.FullPath;
}

/// <summary>Squarified, nested treemap of a <see cref="DirNode"/> tree with category colouring.</summary>
public sealed class TreemapControl : FrameworkElement
{
    public const byte KindContainer = 0, KindFile = 1, KindDirLeaf = 2, KindRest = 3, KindUnknown = 4;

    public static readonly DependencyProperty RootProperty = DependencyProperty.Register(nameof(Root), typeof(DirNode), typeof(TreemapControl),
        new FrameworkPropertyMetadata(null, (d, e) => ((TreemapControl)d).OnRootChanged((DirNode)e.OldValue, (DirNode)e.NewValue)));
    public static readonly DependencyProperty HighlightProperty = DependencyProperty.Register(nameof(Highlight), typeof(int), typeof(TreemapControl),
        new FrameworkPropertyMetadata(-1, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty VersionProperty = DependencyProperty.Register(nameof(Version), typeof(int), typeof(TreemapControl),
        new FrameworkPropertyMetadata(0, (d, _) => ((TreemapControl)d).Invalidate()));
    public static readonly DependencyProperty SelectedProperty = DependencyProperty.Register(nameof(Selected), typeof(object), typeof(TreemapControl),
        new FrameworkPropertyMetadata(null, (d, _) => ((TreemapControl)d).RedrawOverlay()));

    public DirNode Root { get => (DirNode)GetValue(RootProperty); set => SetValue(RootProperty, value); }
    public int Version { get => (int)GetValue(VersionProperty); set => SetValue(VersionProperty, value); }
    public object Selected { get => GetValue(SelectedProperty); set => SetValue(SelectedProperty, value); }
    /// <summary>File category to emphasize (others are dimmed), -1 for none.</summary>
    public int Highlight { get => (int)GetValue(HighlightProperty); set => SetValue(HighlightProperty, value); }

    public event Action<TreemapHit> HoverChanged;
    public event Action<TreemapHit> Clicked;
    public event Action<TreemapHit> DoubleClicked;
    public event Action<TreemapHit> RightClicked;

    private struct Tile
    {
        public Rect R;
        public DirNode Dir;
        public DirNode FileParent;
        public string FileName;
        public long Size;
        public byte Kind;
        public byte Category;
        public bool Header;
    }

    private readonly struct Item
    {
        public readonly long Size;
        public readonly DirNode Dir;
        public readonly int FileIndex;
        public readonly byte Kind;

        public Item(long size, DirNode dir, int fileIndex, byte kind)
        {
            Size = size;
            Dir = dir;
            FileIndex = fileIndex;
            Kind = kind;
        }
    }

    private const int MaxDepth = 9;
    private const double HeaderHeight = 19;
    private readonly List<Tile> _tiles = new();
    private readonly DrawingVisual _overlay = new();
    private bool _dirty = true;
    private Size _laidOutSize;
    private int _hover = -1;
    private Brush[] _catBrushes;
    private bool _brushesDark;
    private Typeface _typeface, _typefaceBold;

    public TreemapControl()
    {
        AddVisualChild(_overlay);
        ClipToBounds = true;
        Cursor = Cursors.Hand;
        App.ThemeChanged += () =>
        {
            _catBrushes = null;
            Invalidate();
        };
    }

    protected override int VisualChildrenCount => 1;
    protected override Visual GetVisualChild(int index) => _overlay;

    protected override HitTestResult HitTestCore(PointHitTestParameters p) =>
        new Rect(RenderSize).Contains(p.HitPoint) ? new PointHitTestResult(this, p.HitPoint) : null;

    private DirNode _zoomOutFrom;

    /// <summary>Zooms into the tile of a sub-folder, or out of the folder we came from.</summary>
    private void OnRootChanged(DirNode oldRoot, DirNode newRoot)
    {
        if (oldRoot != null && newRoot != null && ActualWidth > 0 && ActualHeight > 0)
        {
            if (oldRoot.IsAncestorOf(newRoot) && oldRoot != newRoot)
            {
                var r = TileRect(newRoot);
                if (r.HasValue) AnimateFrom(r.Value.X, r.Value.Y, r.Value.Width / ActualWidth, r.Value.Height / ActualHeight);
            }
            else if (newRoot.IsAncestorOf(oldRoot) && oldRoot != newRoot)
            {
                _zoomOutFrom = oldRoot;
            }
        }
        Invalidate();
    }

    private Rect? TileRect(DirNode d)
    {
        foreach (var t in _tiles)
            if (t.Dir == d && t.Kind != KindFile) return t.R;
        return null;
    }

    private void AnimateFrom(double x, double y, double sx, double sy)
    {
        if (sx <= 0 || sy <= 0 || double.IsNaN(sx) || double.IsNaN(sy)) return;
        var scale = new ScaleTransform(sx, sy);
        var move = new TranslateTransform(x, y);
        RenderTransformOrigin = new Point(0, 0);
        RenderTransform = new TransformGroup { Children = { scale, move } };
        var ease = new System.Windows.Media.Animation.CubicEase { EasingMode = System.Windows.Media.Animation.EasingMode.EaseOut };
        var d = TimeSpan.FromMilliseconds(320);
        System.Windows.Media.Animation.DoubleAnimation A(double from, double to) => new(from, to, d) { EasingFunction = ease };
        scale.BeginAnimation(ScaleTransform.ScaleXProperty, A(sx, 1));
        scale.BeginAnimation(ScaleTransform.ScaleYProperty, A(sy, 1));
        move.BeginAnimation(TranslateTransform.XProperty, A(x, 0));
        move.BeginAnimation(TranslateTransform.YProperty, A(y, 0));
        BeginAnimation(OpacityProperty, new System.Windows.Media.Animation.DoubleAnimation(0.55, 1, TimeSpan.FromMilliseconds(260)));
    }

    private void Invalidate()
    {
        _dirty = true;
        _hover = -1;
        InvalidateVisual();
    }

    protected override void OnRenderSizeChanged(SizeChangedInfo sizeInfo)
    {
        base.OnRenderSizeChanged(sizeInfo);
        Invalidate();
    }

    // ------------------------------------------------------------------ layout

    private void Layout()
    {
        _tiles.Clear();
        var root = Root;
        if (root == null || root.Size <= 0 || ActualWidth < 4 || ActualHeight < 4) return;
        LayoutChildren(root, new Rect(0, 0, ActualWidth, ActualHeight), 0);
    }

    private void LayoutChildren(DirNode dir, Rect r, int depth)
    {
        if (r.Width < 2 || r.Height < 2 || dir.Size <= 0) return;
        double scale = r.Width * r.Height / dir.Size;
        double minSize = 5.0 / scale; // items below ~5 px² are aggregated
        var items = new List<Item>();
        long included = 0;
        int di = 0, fi = 0;
        var dirs = dir.Dirs;
        var files = dir.Files;
        while (di < dirs.Count || fi < files.Length)
        {
            bool takeDir = fi >= files.Length || (di < dirs.Count && dirs[di].Size >= files[fi].Size);
            long size = takeDir ? dirs[di].Size : files[fi].Size;
            if (size < minSize || size <= 0) break;
            items.Add(takeDir ? new Item(size, dirs[di], -1, KindDirLeaf) : new Item(size, null, fi, KindFile));
            included += size;
            if (takeDir) di++; else fi++;
        }
        long rest = dir.Size - included;
        if (rest > 0 && rest * scale >= 2) items.Add(new Item(rest, null, -1, KindRest));
        if (items.Count == 0) return;

        Squarify(items, r, dir, depth);
    }

    private void Squarify(List<Item> items, Rect r, DirNode parent, int depth)
    {
        double total = 0;
        foreach (var it in items) total += it.Size;
        if (total <= 0) return;
        double scale = r.Width * r.Height / total;

        var free = r;
        int i = 0;
        while (i < items.Count)
        {
            double side = Math.Min(free.Width, free.Height);
            if (side <= 0) break;
            int j = i;
            double rowSum = 0, worst = double.MaxValue;
            double first = items[i].Size * scale;
            while (j < items.Count)
            {
                double s = items[j].Size * scale;
                double sum = rowSum + s;
                double w = Math.Max(side * side * first / (sum * sum), sum * sum / (side * side * s));
                if (j > i && w > worst) break;
                rowSum = sum;
                worst = w;
                j++;
            }

            if (free.Width >= free.Height)
            {
                double colW = Math.Min(free.Width, rowSum / free.Height);
                double y = free.Y;
                for (int k = i; k < j; k++)
                {
                    double h = items[k].Size * scale / colW;
                    if (k == j - 1) h = free.Bottom - y;
                    Place(items[k], new Rect(free.X, y, colW, Math.Max(0, h)), parent, depth);
                    y += h;
                }
                free = new Rect(free.X + colW, free.Y, Math.Max(0, free.Width - colW), free.Height);
            }
            else
            {
                double rowH = Math.Min(free.Height, rowSum / free.Width);
                double x = free.X;
                for (int k = i; k < j; k++)
                {
                    double w = items[k].Size * scale / rowH;
                    if (k == j - 1) w = free.Right - x;
                    Place(items[k], new Rect(x, free.Y, Math.Max(0, w), rowH), parent, depth);
                    x += w;
                }
                free = new Rect(free.X, free.Y + rowH, free.Width, Math.Max(0, free.Height - rowH));
            }
            i = j;
        }
    }

    private void Place(Item it, Rect rc, DirNode parent, int depth)
    {
        if (rc.Width < 0.5 || rc.Height < 0.5) return;
        switch (it.Kind)
        {
            case KindFile:
            {
                var f = parent.Files[it.FileIndex];
                _tiles.Add(new Tile { R = rc, FileParent = parent, FileName = f.Name, Size = f.Size, Kind = KindFile, Category = f.Category });
                break;
            }
            case KindRest:
                _tiles.Add(new Tile { R = rc, FileParent = parent, Size = it.Size, Kind = KindRest });
                break;
            default:
            {
                var d = it.Dir;
                if (d.IsVirtual)
                {
                    _tiles.Add(new Tile { R = rc, Dir = d, Size = d.Size, Kind = KindUnknown });
                    break;
                }
                bool canNest = depth < MaxDepth && rc.Width >= 26 && rc.Height >= 22 && (d.Dirs.Count > 0 || d.Files.Length > 0);
                if (!canNest)
                {
                    _tiles.Add(new Tile { R = rc, Dir = d, Size = d.Size, Kind = KindDirLeaf, Category = d.Dominant });
                    break;
                }
                bool header = rc.Width >= 70 && rc.Height >= 46;
                _tiles.Add(new Tile { R = rc, Dir = d, Size = d.Size, Kind = KindContainer, Header = header });
                var inner = header
                    ? new Rect(rc.X + 3, rc.Y + HeaderHeight, Math.Max(0, rc.Width - 6), Math.Max(0, rc.Height - HeaderHeight - 3))
                    : new Rect(rc.X + 2, rc.Y + 2, Math.Max(0, rc.Width - 4), Math.Max(0, rc.Height - 4));
                LayoutChildren(d, inner, depth + 1);
                break;
            }
        }
    }

    // ------------------------------------------------------------------ rendering

    private Brush Res(string key, Brush fallback) => TryFindResource(key) as Brush ?? fallback;

    private void EnsureBrushes()
    {
        if (_catBrushes != null && _brushesDark == App.IsDark) return;
        _brushesDark = App.IsDark;
        _catBrushes = FileCategories.All.Select(c => (Brush)Cushion(c.Color)).ToArray();
        _typeface = new Typeface(new FontFamily("Segoe UI"), FontStyles.Normal, FontWeights.Normal, FontStretches.Normal);
        _typefaceBold = new Typeface(new FontFamily("Segoe UI"), FontStyles.Normal, FontWeights.SemiBold, FontStretches.Normal);
    }

    private static LinearGradientBrush Cushion(Color c)
    {
        var b = new LinearGradientBrush
        {
            StartPoint = new Point(0, 0),
            EndPoint = new Point(1, 1),
            GradientStops =
            {
                new GradientStop(Mix(c, Colors.White, 0.22), 0),
                new GradientStop(c, 0.55),
                new GradientStop(Mix(c, Colors.Black, 0.18), 1)
            }
        };
        b.Freeze();
        return b;
    }

    private static Color Mix(Color a, Color b, double t) => Color.FromArgb(255,
        (byte)(a.R + (b.R - a.R) * t), (byte)(a.G + (b.G - a.G) * t), (byte)(a.B + (b.B - a.B) * t));

    protected override void OnRender(DrawingContext dc)
    {
        var size = RenderSize;
        if (_dirty || size != _laidOutSize)
        {
            Layout();
            _dirty = false;
            _laidOutSize = size;
            if (_zoomOutFrom != null)
            {
                var from = TileRect(_zoomOutFrom);
                _zoomOutFrom = null;
                if (from is { Width: > 1, Height: > 1 } r)
                {
                    double sx = size.Width / r.Width, sy = size.Height / r.Height;
                    AnimateFrom(-r.X * sx, -r.Y * sy, sx, sy);
                }
            }
        }
        EnsureBrushes();

        var surface = Res("Brush.Surface", Brushes.Black);
        dc.DrawRectangle(surface, null, new Rect(size));
        if (_tiles.Count == 0)
        {
            RedrawOverlay();
            return;
        }

        bool dark = App.IsDark;
        var containerFill = new SolidColorBrush(dark ? Color.FromArgb(14, 255, 255, 255) : Color.FromArgb(10, 0, 0, 0));
        containerFill.Freeze();
        var containerPen = new Pen(new SolidColorBrush(dark ? Color.FromArgb(28, 255, 255, 255) : Color.FromArgb(26, 0, 0, 0)), 1);
        containerPen.Freeze();
        var headerText = Res("Brush.TextSecondary", Brushes.Gray);
        var headerMuted = Res("Brush.TextMuted", Brushes.Gray);
        var leafBrush = Cushion(dark ? Color.FromRgb(0x4A, 0x55, 0x6B) : Color.FromRgb(0x9A, 0xA4, 0xB8));
        var restBrush = new SolidColorBrush(dark ? Color.FromArgb(40, 255, 255, 255) : Color.FromArgb(30, 0, 0, 0));
        restBrush.Freeze();
        var unknownBrush = Hatch(dark);
        double dpi = VisualTreeHelper.GetDpi(this).PixelsPerDip;
        int labels = 0;
        int highlight = Highlight;

        foreach (var t in _tiles)
        {
            var r = t.R;
            bool dim = highlight >= 0 && (t.Kind == KindFile || t.Kind == KindDirLeaf) && t.Category != highlight;
            if (dim) dc.PushOpacity(0.16);
            switch (t.Kind)
            {
                case KindContainer:
                    dc.DrawRoundedRectangle(containerFill, containerPen, Inset(r, 0.5), 4, 4);
                    if (t.Header && labels < 600)
                    {
                        labels++;
                        DrawText(dc, t.Dir.Name, _typefaceBold, 11, headerText, r.X + 6, r.Y + 3, r.Width - 12, dpi,
                            Fmt.Size(t.Size), headerMuted);
                    }
                    break;
                case KindFile:
                {
                    var tile = Inset(r, 0.6);
                    if (tile.Width <= 0 || tile.Height <= 0) break;
                    var brush = _catBrushes[t.Category < _catBrushes.Length ? t.Category : _catBrushes.Length - 1];
                    if (tile.Width > 8 && tile.Height > 8) dc.DrawRoundedRectangle(brush, null, tile, 3, 3);
                    else dc.DrawRectangle(brush, null, tile);
                    if (labels < 600 && tile.Width >= 56 && tile.Height >= 20)
                    {
                        labels++;
                        DrawTileLabel(dc, t.FileName, Fmt.Size(t.Size), tile, dpi);
                    }
                    break;
                }
                case KindDirLeaf:
                {
                    var tile = Inset(r, 0.6);
                    if (tile.Width <= 0 || tile.Height <= 0) break;
                    var brush = t.Category < _catBrushes.Length ? _catBrushes[t.Category] : leafBrush;
                    if (tile.Width > 8 && tile.Height > 8) dc.DrawRoundedRectangle(brush, null, tile, 3, 3);
                    else dc.DrawRectangle(brush, null, tile);
                    if (labels < 600 && tile.Width >= 56 && tile.Height >= 20)
                    {
                        labels++;
                        DrawTileLabel(dc, t.Dir.Name, Fmt.Size(t.Size), tile, dpi);
                    }
                    break;
                }
                case KindRest:
                {
                    var tile = Inset(r, 0.6);
                    if (tile.Width > 0 && tile.Height > 0) dc.DrawRoundedRectangle(restBrush, null, tile, 3, 3);
                    break;
                }
                case KindUnknown:
                {
                    var tile = Inset(r, 0.6);
                    if (tile.Width <= 0 || tile.Height <= 0) break;
                    dc.DrawRoundedRectangle(unknownBrush, containerPen, tile, 4, 4);
                    if (tile.Width >= 90 && tile.Height >= 36)
                        DrawText(dc, t.Dir.Name, _typefaceBold, 11.5, headerText, tile.X + 8, tile.Y + 6, tile.Width - 16, dpi,
                            Fmt.Size(t.Size), headerMuted);
                    break;
                }
            }
            if (dim) dc.Pop();
        }
        RedrawOverlay();
    }

    private static Brush Hatch(bool dark)
    {
        var line = new SolidColorBrush(dark ? Color.FromArgb(40, 255, 255, 255) : Color.FromArgb(36, 0, 0, 0));
        var geo = new GeometryGroup();
        geo.Children.Add(new LineGeometry(new Point(0, 8), new Point(8, 0)));
        geo.Children.Add(new LineGeometry(new Point(-2, 2), new Point(2, -2)));
        geo.Children.Add(new LineGeometry(new Point(6, 10), new Point(10, 6)));
        var drawing = new GeometryDrawing(null, new Pen(line, 1.2), geo);
        var b = new DrawingBrush(drawing)
        {
            TileMode = TileMode.Tile,
            Viewport = new Rect(0, 0, 8, 8),
            ViewportUnits = BrushMappingMode.Absolute,
            Viewbox = new Rect(0, 0, 8, 8),
            ViewboxUnits = BrushMappingMode.Absolute
        };
        b.Freeze();
        return b;
    }

    private static Rect Inset(Rect r, double d) =>
        new(r.X + d, r.Y + d, Math.Max(0, r.Width - 2 * d), Math.Max(0, r.Height - 2 * d));

    private void DrawTileLabel(DrawingContext dc, string name, string size, Rect tile, double dpi)
    {
        var fg = Brushes.White;
        var ft = Make(name, _typefaceBold, 11, fg, tile.Width - 10, dpi);
        if (tile.Height >= 36)
        {
            dc.DrawText(ft, new Point(tile.X + 6, tile.Y + 4));
            var fs = Make(size, _typeface, 10.5, fg, tile.Width - 10, dpi);
            fs.SetForegroundBrush(new SolidColorBrush(Color.FromArgb(200, 255, 255, 255)));
            dc.DrawText(fs, new Point(tile.X + 6, tile.Y + 19));
        }
        else
        {
            dc.DrawText(ft, new Point(tile.X + 6, tile.Y + (tile.Height - ft.Height) / 2));
        }
    }

    private void DrawText(DrawingContext dc, string text, Typeface tf, double size, Brush brush, double x, double y, double maxW, double dpi,
        string suffix, Brush suffixBrush)
    {
        if (maxW < 12) return;
        var fs = Make(suffix, _typeface, size - 0.5, suffixBrush, maxW, dpi);
        double nameW = maxW - fs.Width - 8;
        if (nameW >= 24)
        {
            var ft = Make(text, tf, size, brush, nameW, dpi);
            dc.DrawText(ft, new Point(x, y));
            dc.DrawText(fs, new Point(x + Math.Min(ft.WidthIncludingTrailingWhitespace, nameW) + 8, y + 0.5));
        }
        else
        {
            dc.DrawText(Make(text, tf, size, brush, maxW, dpi), new Point(x, y));
        }
    }

    private static FormattedText Make(string text, Typeface tf, double size, Brush brush, double maxW, double dpi)
    {
        var ft = new FormattedText(text ?? "", Fmt.Ru, FlowDirection.LeftToRight, tf, size, brush, dpi)
        {
            MaxLineCount = 1,
            Trimming = TextTrimming.CharacterEllipsis
        };
        if (maxW > 1) ft.MaxTextWidth = maxW;
        return ft;
    }

    private void RedrawOverlay()
    {
        using var dc = _overlay.RenderOpen();
        var accent = Res("Brush.Accent", Brushes.MediumSlateBlue);
        var sel = FindSelected();
        if (sel >= 0)
        {
            var r = Inset(_tiles[sel].R, 1);
            dc.DrawRoundedRectangle(null, new Pen(accent, 2.2), r, 4, 4);
        }
        if (_hover >= 0 && _hover < _tiles.Count)
        {
            var t = _tiles[_hover];
            var r = Inset(t.R, 0.6);
            var fill = new SolidColorBrush(Color.FromArgb(t.Kind == KindContainer ? (byte)18 : (byte)46, 255, 255, 255));
            var pen = new Pen(new SolidColorBrush(App.IsDark ? Color.FromArgb(220, 255, 255, 255) : Color.FromArgb(200, 20, 20, 30)), 1.4);
            dc.DrawRoundedRectangle(fill, pen, r, 3, 3);
        }
    }

    private int FindSelected()
    {
        var s = Selected;
        if (s == null) return -1;
        for (int i = 0; i < _tiles.Count; i++)
        {
            var t = _tiles[i];
            if (s is DirNode d && t.Dir == d && t.Kind != KindFile) return i;
            if (s is FileRef f && t.Kind == KindFile && t.FileParent == f.Parent &&
                string.Equals(t.FileName, f.Name, StringComparison.OrdinalIgnoreCase)) return i;
        }
        return -1;
    }

    // ------------------------------------------------------------------ input

    private int HitIndex(Point p)
    {
        for (int i = _tiles.Count - 1; i >= 0; i--)
            if (_tiles[i].R.Contains(p)) return i;
        return -1;
    }

    private TreemapHit ToHit(int i)
    {
        if (i < 0 || i >= _tiles.Count) return null;
        var t = _tiles[i];
        return new TreemapHit
        {
            Dir = t.Dir, FileParent = t.FileParent, FileName = t.FileName, Size = t.Size, Kind = t.Kind, Category = t.Category
        };
    }

    protected override void OnMouseMove(MouseEventArgs e)
    {
        base.OnMouseMove(e);
        int i = HitIndex(e.GetPosition(this));
        if (i == _hover) return;
        _hover = i;
        RedrawOverlay();
        HoverChanged?.Invoke(ToHit(i));
    }

    protected override void OnMouseLeave(MouseEventArgs e)
    {
        base.OnMouseLeave(e);
        _hover = -1;
        RedrawOverlay();
        HoverChanged?.Invoke(null);
    }

    protected override void OnMouseLeftButtonDown(MouseButtonEventArgs e)
    {
        base.OnMouseLeftButtonDown(e);
        var hit = ToHit(HitIndex(e.GetPosition(this)));
        if (hit == null) return;
        if (e.ClickCount >= 2) DoubleClicked?.Invoke(hit);
        else Clicked?.Invoke(hit);
        e.Handled = true;
    }

    protected override void OnMouseRightButtonUp(MouseButtonEventArgs e)
    {
        base.OnMouseRightButtonUp(e);
        var hit = ToHit(HitIndex(e.GetPosition(this)));
        if (hit != null) RightClicked?.Invoke(hit);
        e.Handled = true;
    }
}
