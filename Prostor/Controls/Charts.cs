using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;

namespace Prostor.Controls;

/// <summary>Circular progress ring.</summary>
public sealed class RingChart : FrameworkElement
{
    public static readonly DependencyProperty ValueProperty = DependencyProperty.Register(nameof(Value), typeof(double), typeof(RingChart),
        new FrameworkPropertyMetadata(0.0, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty ThicknessProperty = DependencyProperty.Register(nameof(Thickness), typeof(double), typeof(RingChart),
        new FrameworkPropertyMetadata(10.0, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty TrackProperty = DependencyProperty.Register(nameof(Track), typeof(Brush), typeof(RingChart),
        new FrameworkPropertyMetadata(Brushes.Gray, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty FillProperty = DependencyProperty.Register(nameof(Fill), typeof(Brush), typeof(RingChart),
        new FrameworkPropertyMetadata(Brushes.SlateBlue, FrameworkPropertyMetadataOptions.AffectsRender));

    public double Value { get => (double)GetValue(ValueProperty); set => SetValue(ValueProperty, value); }
    public double Thickness { get => (double)GetValue(ThicknessProperty); set => SetValue(ThicknessProperty, value); }
    public Brush Track { get => (Brush)GetValue(TrackProperty); set => SetValue(TrackProperty, value); }
    public Brush Fill { get => (Brush)GetValue(FillProperty); set => SetValue(FillProperty, value); }

    protected override void OnRender(DrawingContext dc)
    {
        double size = Math.Min(ActualWidth, ActualHeight);
        if (size <= 0) return;
        double t = Thickness;
        double r = (size - t) / 2;
        var c = new Point(ActualWidth / 2, ActualHeight / 2);
        dc.DrawEllipse(null, new Pen(Track, t), c, r, r);

        double v = Math.Clamp(Value, 0, 1);
        if (v <= 0.001) return;
        var pen = new Pen(Fill, t) { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round };
        if (v >= 0.999)
        {
            dc.DrawEllipse(null, pen, c, r, r);
            return;
        }
        double angle = v * 360;
        double a0 = -90 * Math.PI / 180, a1 = (angle - 90) * Math.PI / 180;
        var p0 = new Point(c.X + r * Math.Cos(a0), c.Y + r * Math.Sin(a0));
        var p1 = new Point(c.X + r * Math.Cos(a1), c.Y + r * Math.Sin(a1));
        var g = new StreamGeometry();
        using (var ctx = g.Open())
        {
            ctx.BeginFigure(p0, false, false);
            ctx.ArcTo(p1, new Size(r, r), 0, angle > 180, SweepDirection.Clockwise, true, false);
        }
        g.Freeze();
        dc.DrawGeometry(null, pen, g);
    }
}

/// <summary>Progress bar with a determinate (0..1) and an animated indeterminate mode.</summary>
public sealed class ProgressLine : FrameworkElement
{
    public static readonly DependencyProperty ValueProperty = DependencyProperty.Register(nameof(Value), typeof(double), typeof(ProgressLine),
        new FrameworkPropertyMetadata(0.0, FrameworkPropertyMetadataOptions.AffectsRender, (d, e) => ((ProgressLine)d).OnValueChanged((double)e.NewValue)));
    public static readonly DependencyProperty IsIndeterminateProperty = DependencyProperty.Register(nameof(IsIndeterminate), typeof(bool), typeof(ProgressLine),
        new FrameworkPropertyMetadata(false, FrameworkPropertyMetadataOptions.AffectsRender, (d, _) => ((ProgressLine)d).UpdateAnimation()));
    public static readonly DependencyProperty TrackProperty = DependencyProperty.Register(nameof(Track), typeof(Brush), typeof(ProgressLine),
        new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty FillProperty = DependencyProperty.Register(nameof(Fill), typeof(Brush), typeof(ProgressLine),
        new FrameworkPropertyMetadata(Brushes.SlateBlue, FrameworkPropertyMetadataOptions.AffectsRender));

    public double Value { get => (double)GetValue(ValueProperty); set => SetValue(ValueProperty, value); }
    public bool IsIndeterminate { get => (bool)GetValue(IsIndeterminateProperty); set => SetValue(IsIndeterminateProperty, value); }
    public Brush Track { get => (Brush)GetValue(TrackProperty); set => SetValue(TrackProperty, value); }
    public Brush Fill { get => (Brush)GetValue(FillProperty); set => SetValue(FillProperty, value); }

    private bool _animating;
    private readonly System.Diagnostics.Stopwatch _clock = System.Diagnostics.Stopwatch.StartNew();
    private double _shown;      // smoothly animated value in determinate mode
    private double _target;

    public ProgressLine()
    {
        IsVisibleChanged += (_, _) => UpdateAnimation();
        Unloaded += (_, _) => Stop();
        Loaded += (_, _) => UpdateAnimation();
    }

    protected override Size MeasureOverride(Size availableSize) => new(0, double.IsNaN(Height) ? 6 : Height);

    private void OnValueChanged(double v)
    {
        _target = Math.Clamp(v, 0, 1);
        if (_target < _shown) _shown = _target;
        UpdateAnimation();
    }

    private void UpdateAnimation()
    {
        bool need = IsVisible && (IsIndeterminate || Math.Abs(_shown - _target) > 0.001);
        if (need && !_animating)
        {
            CompositionTarget.Rendering += OnFrame;
            _animating = true;
        }
        else if (!need) Stop();
        InvalidateVisual();
    }

    private void Stop()
    {
        if (!_animating) return;
        CompositionTarget.Rendering -= OnFrame;
        _animating = false;
    }

    private void OnFrame(object sender, EventArgs e)
    {
        if (!IsIndeterminate)
        {
            _shown += (_target - _shown) * 0.18;
            if (Math.Abs(_target - _shown) < 0.002)
            {
                _shown = _target;
                Stop();
            }
        }
        InvalidateVisual();
    }

    protected override void OnRender(DrawingContext dc)
    {
        double w = ActualWidth, h = ActualHeight;
        if (w <= 0 || h <= 0) return;
        double r = h / 2;
        if (Track != null) dc.DrawRoundedRectangle(Track, null, new Rect(0, 0, w, h), r, r);

        if (IsIndeterminate)
        {
            // a segment that sweeps from left to right with ease-in-out
            double t = _clock.Elapsed.TotalSeconds % 1.6 / 1.6;
            double eased = t < 0.5 ? 2 * t * t : 1 - Math.Pow(-2 * t + 2, 2) / 2;
            double seg = Math.Max(w * 0.28, 40);
            double x = -seg + (w + seg) * eased;
            double x0 = Math.Max(0, x), x1 = Math.Min(w, x + seg);
            if (x1 - x0 > 0.5)
            {
                dc.PushClip(new RectangleGeometry(new Rect(0, 0, w, h), r, r));
                dc.DrawRoundedRectangle(Fill, null, new Rect(x0, 0, x1 - x0, h), r, r);
                dc.Pop();
            }
            return;
        }

        double v = _animating ? _shown : Math.Clamp(Value, 0, 1);
        if (!_animating) _shown = v;
        if (v <= 0) return;
        dc.DrawRoundedRectangle(Fill, null, new Rect(0, 0, Math.Min(w, Math.Max(h, w * v)), h), r, r);
    }
}

/// <summary>Thin horizontal bar (0..1) with rounded ends.</summary>
public sealed class MiniBar : FrameworkElement
{
    public static readonly DependencyProperty ValueProperty = DependencyProperty.Register(nameof(Value), typeof(double), typeof(MiniBar),
        new FrameworkPropertyMetadata(0.0, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty TrackProperty = DependencyProperty.Register(nameof(Track), typeof(Brush), typeof(MiniBar),
        new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty FillProperty = DependencyProperty.Register(nameof(Fill), typeof(Brush), typeof(MiniBar),
        new FrameworkPropertyMetadata(Brushes.SlateBlue, FrameworkPropertyMetadataOptions.AffectsRender));

    public double Value { get => (double)GetValue(ValueProperty); set => SetValue(ValueProperty, value); }
    public Brush Track { get => (Brush)GetValue(TrackProperty); set => SetValue(TrackProperty, value); }
    public Brush Fill { get => (Brush)GetValue(FillProperty); set => SetValue(FillProperty, value); }

    public MiniBar()
    {
        SnapsToDevicePixels = true;
    }

    protected override Size MeasureOverride(Size availableSize) => new(0, double.IsNaN(Height) ? 6 : Height);

    protected override void OnRender(DrawingContext dc)
    {
        double w = ActualWidth, h = ActualHeight;
        if (w <= 0 || h <= 0) return;
        double r = h / 2;
        if (Track != null) dc.DrawRoundedRectangle(Track, null, new Rect(0, 0, w, h), r, r);
        double v = Math.Clamp(Value, 0, 1);
        if (v <= 0) return;
        double fw = Math.Max(h, w * v);
        dc.DrawRoundedRectangle(Fill, null, new Rect(0, 0, Math.Min(w, fw), h), r, r);
    }
}

/// <summary>Vertical bars (e.g. boot durations); the last bar is highlighted. Hover shows the matching label.</summary>
public sealed class BarChart : FrameworkElement
{
    public static readonly DependencyProperty ValuesProperty = DependencyProperty.Register(nameof(Values), typeof(IList<double>), typeof(BarChart),
        new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty LabelsProperty = DependencyProperty.Register(nameof(Labels), typeof(IList<string>), typeof(BarChart),
        new FrameworkPropertyMetadata(null));
    public static readonly DependencyProperty FillProperty = DependencyProperty.Register(nameof(Fill), typeof(Brush), typeof(BarChart),
        new FrameworkPropertyMetadata(Brushes.SlateBlue, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty HighlightProperty = DependencyProperty.Register(nameof(Highlight), typeof(Brush), typeof(BarChart),
        new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));

    public IList<double> Values { get => (IList<double>)GetValue(ValuesProperty); set => SetValue(ValuesProperty, value); }
    public IList<string> Labels { get => (IList<string>)GetValue(LabelsProperty); set => SetValue(LabelsProperty, value); }
    public Brush Fill { get => (Brush)GetValue(FillProperty); set => SetValue(FillProperty, value); }
    public Brush Highlight { get => (Brush)GetValue(HighlightProperty); set => SetValue(HighlightProperty, value); }

    private int _hover = -1;

    public BarChart()
    {
        ToolTipService.SetInitialShowDelay(this, 0);
    }

    private (double slot, double bar) Geometry(int n)
    {
        double slot = ActualWidth / Math.Max(1, n);
        return (slot, Math.Max(2, Math.Min(26, slot * 0.62)));
    }

    protected override void OnMouseMove(System.Windows.Input.MouseEventArgs e)
    {
        base.OnMouseMove(e);
        var v = Values;
        if (v == null || v.Count == 0) return;
        var (slot, _) = Geometry(v.Count);
        int i = Math.Clamp((int)(e.GetPosition(this).X / slot), 0, v.Count - 1);
        if (i == _hover) return;
        _hover = i;
        ToolTip = Labels != null && i < Labels.Count ? Labels[i] : null;
        InvalidateVisual();
    }

    protected override void OnMouseLeave(System.Windows.Input.MouseEventArgs e)
    {
        base.OnMouseLeave(e);
        _hover = -1;
        InvalidateVisual();
    }

    protected override void OnRender(DrawingContext dc)
    {
        dc.DrawRectangle(Brushes.Transparent, null, new Rect(0, 0, ActualWidth, ActualHeight));
        var v = Values;
        if (v == null || v.Count == 0 || ActualHeight <= 0) return;
        double max = Math.Max(0.001, v.Max());
        var (slot, bar) = Geometry(v.Count);
        for (int i = 0; i < v.Count; i++)
        {
            double h = Math.Max(3, ActualHeight * v[i] / max);
            double x = i * slot + (slot - bar) / 2;
            var brush = i == v.Count - 1 && Highlight != null ? Highlight : Fill;
            if (i == _hover) dc.PushOpacity(0.75);
            dc.DrawRoundedRectangle(brush, null, new Rect(x, ActualHeight - h, bar, h), Math.Min(4, bar / 2), Math.Min(4, bar / 2));
            if (i == _hover) dc.Pop();
        }
    }
}

/// <summary>Line with a soft filled area (e.g. free space over time).</summary>
public sealed class Sparkline : FrameworkElement
{
    public static readonly DependencyProperty ValuesProperty = DependencyProperty.Register(nameof(Values), typeof(IList<double>), typeof(Sparkline),
        new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty StrokeProperty = DependencyProperty.Register(nameof(Stroke), typeof(Brush), typeof(Sparkline),
        new FrameworkPropertyMetadata(Brushes.SlateBlue, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty MinValueProperty = DependencyProperty.Register(nameof(MinValue), typeof(double), typeof(Sparkline),
        new FrameworkPropertyMetadata(double.NaN, FrameworkPropertyMetadataOptions.AffectsRender));
    public static readonly DependencyProperty MaxValueProperty = DependencyProperty.Register(nameof(MaxValue), typeof(double), typeof(Sparkline),
        new FrameworkPropertyMetadata(double.NaN, FrameworkPropertyMetadataOptions.AffectsRender));

    public IList<double> Values { get => (IList<double>)GetValue(ValuesProperty); set => SetValue(ValuesProperty, value); }
    public Brush Stroke { get => (Brush)GetValue(StrokeProperty); set => SetValue(StrokeProperty, value); }
    public double MinValue { get => (double)GetValue(MinValueProperty); set => SetValue(MinValueProperty, value); }
    public double MaxValue { get => (double)GetValue(MaxValueProperty); set => SetValue(MaxValueProperty, value); }

    protected override void OnRender(DrawingContext dc)
    {
        var v = Values;
        double w = ActualWidth, h = ActualHeight;
        if (v == null || v.Count < 2 || w <= 0 || h <= 0) return;
        double min = double.IsNaN(MinValue) ? v.Min() : MinValue;
        double max = double.IsNaN(MaxValue) ? v.Max() : MaxValue;
        if (max - min < 1e-9)
        {
            max += 1;
            min -= 1;
        }
        double pad = 2;
        Point P(int i) => new(i * (w - 1) / (v.Count - 1), pad + (h - 2 * pad) * (1 - (v[i] - min) / (max - min)));

        var line = new StreamGeometry();
        using (var ctx = line.Open())
        {
            ctx.BeginFigure(P(0), false, false);
            for (int i = 1; i < v.Count; i++) ctx.LineTo(P(i), true, true);
        }
        line.Freeze();
        var area = new StreamGeometry();
        using (var ctx = area.Open())
        {
            ctx.BeginFigure(new Point(0, h), true, true);
            for (int i = 0; i < v.Count; i++) ctx.LineTo(P(i), true, false);
            ctx.LineTo(new Point(w - 1, h), true, false);
        }
        area.Freeze();

        if (Stroke is SolidColorBrush sc)
        {
            var c = sc.Color;
            var fill = new LinearGradientBrush(Color.FromArgb(70, c.R, c.G, c.B), Color.FromArgb(0, c.R, c.G, c.B), 90);
            fill.Freeze();
            dc.DrawGeometry(fill, null, area);
        }
        dc.DrawGeometry(null, new Pen(Stroke, 2) { LineJoin = PenLineJoin.Round, StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round }, line);
        var last = P(v.Count - 1);
        dc.DrawEllipse(Stroke, null, last, 3, 3);
    }
}
