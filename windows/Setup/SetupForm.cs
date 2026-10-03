namespace WhooshSetup;

sealed class SetupForm : Form
{
    readonly Label _status;
    readonly ProgressBar _progress;
    readonly Button _install;
    readonly CheckBox _openAfter;
    readonly Label _path;
    bool _installed;

    public SetupForm()
    {
        Text = "Whoosh Setup";
        FormBorderStyle = FormBorderStyle.FixedDialog;
        MaximizeBox = false;
        MinimizeBox = false;
        StartPosition = FormStartPosition.CenterScreen;
        ClientSize = new Size(460, 280);
        try
        {
            Icon = System.Drawing.Icon.ExtractAssociatedIcon(Application.ExecutablePath);
        }
        catch
        {
            // The window still opens without an icon.
        }

        var mark = new PictureBox
        {
            Bounds = new Rectangle(24, 20, 56, 56),
            Image = DrawMark(56),
            SizeMode = PictureBoxSizeMode.CenterImage,
        };
        var title = new Label
        {
            Text = "Install Whoosh",
            Font = new Font("Segoe UI", 16f, FontStyle.Bold),
            AutoSize = true,
            Location = new Point(96, 24),
        };
        var body = new Label
        {
            Text = "Send files, photos, and videos to other Whoosh computers and Android Quick Share. Whoosh is installed for this user.",
            Location = new Point(96, 58),
            Size = new Size(340, 48),
        };
        _path = new Label
        {
            Text = Installer.InstallDirectory,
            ForeColor = SystemColors.GrayText,
            Location = new Point(24, 128),
            Size = new Size(412, 32),
        };
        _progress = new ProgressBar
        {
            Location = new Point(24, 168),
            Size = new Size(412, 8),
            Style = ProgressBarStyle.Marquee,
            Visible = false,
        };
        _status = new Label
        {
            Text = Directory.Exists(Installer.InstallDirectory) ? "Whoosh is already installed. Installing again replaces the program files." : "Windows may ask for network permission the first time Whoosh listens.",
            Location = new Point(24, 184),
            Size = new Size(412, 36),
        };
        _openAfter = new CheckBox
        {
            Text = "Open Whoosh",
            Checked = true,
            AutoSize = true,
            Location = new Point(24, 232),
        };
        _install = new Button
        {
            Text = Directory.Exists(Installer.InstallDirectory) ? "Reinstall" : "Install",
            Location = new Point(344, 228),
            Size = new Size(92, 32),
        };
        _install.Click += async (_, _) =>
        {
            if (_installed)
            {
                Close();
                return;
            }

            await InstallAsync();
        };
        AcceptButton = _install;
        Controls.AddRange([mark, title, body, _path, _progress, _status, _openAfter, _install]);
    }

    async Task InstallAsync()
    {
        _install.Enabled = false;
        _openAfter.Enabled = false;
        _progress.Visible = true;
        try
        {
            var progress = new Progress<string>(text => _status.Text = text);
            var directory = await Task.Run(() => Installer.Install(progress));
            if (_openAfter.Checked)
            {
                Installer.Launch(directory);
                Close();
                return;
            }

            _installed = true;
            _status.Text = "Whoosh is installed.";
            _progress.Visible = false;
            _install.Text = "Close";
            _install.Enabled = true;
        }
        catch (Exception error)
        {
            _progress.Visible = false;
            _install.Enabled = true;
            _openAfter.Enabled = true;
            MessageBox.Show(this, error.Message, "Whoosh Setup", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    static Bitmap DrawMark(int size)
    {
        var bitmap = new Bitmap(size, size);
        using var graphics = Graphics.FromImage(bitmap);
        graphics.SmoothingMode = System.Drawing.Drawing2D.SmoothingMode.AntiAlias;
        graphics.Clear(Color.Transparent);
        var arcs = new (float Radius, int Alpha, float Width)[]
        {
            (0.40f, 70, 0.072f),
            (0.285f, 150, 0.078f),
            (0.165f, 245, 0.086f),
        };
        foreach (var arc in arcs)
        {
            var radius = size * arc.Radius;
            using var pen = new Pen(Color.FromArgb(arc.Alpha, 20, 24, 22), size * arc.Width);
            pen.StartCap = System.Drawing.Drawing2D.LineCap.Round;
            pen.EndCap = System.Drawing.Drawing2D.LineCap.Round;
            var cx = size * 0.50f;
            var cy = size * 0.62f;
            graphics.DrawArc(pen, cx - radius, cy - radius, radius * 2, radius * 2, 206, 132);
        }

        return bitmap;
    }
}
