// Program.cs - ODR desktop app entry point.
//   ODR.exe                    normal use
//   ODR.exe --autotest <dir>   unattended proof run (CI): drives the real window, saves screenshots
//                              and autotest.txt to <dir>, exit code 0 = everything passed.
//   ODR.exe --selftest-license  runs the remote-lock state-table tests (no network), prints the report.
using System;
using System.Windows.Forms;

namespace OdrApp
{
    static class Program
    {
        [STAThread]
        static int Main(string[] args)
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            string autotest = null;
            for (int i = 0; i < args.Length; i++)
            {
                if (args[i] == "--autotest" && i + 1 < args.Length) autotest = args[i + 1];
                if (args[i] == "--selftest-license")
                {
                    int code = Tests.LicenseTests.Run(out string report);
                    Console.WriteLine(report);
                    try { System.IO.File.WriteAllText("license-selftest.txt", report, new System.Text.UTF8Encoding(true)); } catch { }
                    return code;
                }
            }
            var f = new MainForm(autotest);
            Application.Run(f);
            return f.ExitCode;
        }
    }
}
