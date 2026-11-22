// The Windows bootstrap's protocol side (see transport.rs: WINDOWS_BOOT_CS). C# 5, compiled on
// the host by Windows PowerShell's Add-Type (the .NET Framework compiler every Windows has).
using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

static class MarsadBoot
{
    [DllImport("kernel32.dll")]
    static extern IntPtr GetStdHandle(int which);

    [DllImport("ntdll.dll")]
    static extern int NtQueryInformationFile(IntPtr file, long[] status, out int info, int length, int infoClass);

    static Stream input, output = Console.OpenStandardOutput();

    // OpenSSH for Windows hands a command a pipe opened for overlapped I/O. The .NET Framework's
    // console stream reads it synchronously, which returns what is already there and then never
    // completes; a FileStream opened as asynchronous reads it properly. A synchronous handle
    // (FILE_SYNCHRONOUS_IO_ALERT / _NONALERT in its mode) is read synchronously. Unbuffered
    // either way: the bridge started after this program gets every byte it did not read.
    static Stream OpenInput()
    {
        IntPtr h = GetStdHandle(-10);
        int mode;
        bool synchronous = NtQueryInformationFile(h, new long[2], out mode, 4, 16) != 0 || (mode & 0x30) != 0;
        return new FileStream(new SafeFileHandle(h, false), FileAccess.Read, 1, !synchronous);
    }

    static void Say(string text)
    {
        byte[] b = Encoding.ASCII.GetBytes(text + "\n");
        output.Write(b, 0, b.Length);
        output.Flush();
    }

    static string ReadLine()
    {
        var s = new StringBuilder();
        for (;;)
        {
            int c = input.ReadByte();
            if (c < 0) throw new Exception("the connection closed during the bootstrap");
            if (c == '\n') return s.ToString();
            s.Append((char)c);
        }
    }

    // Runs the installed helper with its own stdin and stdout (not the connection's).
    static string Helper(string exe, string args)
    {
        var info = new ProcessStartInfo(exe, args);
        info.UseShellExecute = false;
        info.CreateNoWindow = true;
        info.RedirectStandardInput = true;
        info.RedirectStandardOutput = true;
        using (var p = Process.Start(info))
        {
            p.StandardInput.Close();
            string text = p.StandardOutput.ReadToEnd();
            p.WaitForExit();
            return text;
        }
    }

    static int Main()
    {
        try
        {
            input = OpenInput();
            string dir = Path.Combine(Environment.GetEnvironmentVariable("LOCALAPPDATA"), "Marsad", "bin");
            string exe = Path.Combine(dir, "marsad-agent.exe");
            int pid = Process.GetCurrentProcess().Id;
            string version = "none";
            if (File.Exists(exe))
            {
                version = Helper(exe, "--version").Replace(" ", "").Replace("\r", "").Replace("\n", "");
                if (version == "") version = "none";
            }
            Say("MARSAD-BOOT Windows " + Environment.GetEnvironmentVariable("PROCESSOR_ARCHITECTURE") + " " + version);
            string[] cmd = ReadLine().Split(' ');
            if (cmd[0] == "UPLOAD")
            {
                long left = long.Parse(cmd[1]);
                string tmp = exe + ".new." + pid;
                try
                {
                    using (var f = File.Create(tmp))
                    {
                        var buf = new byte[65536];
                        while (left > 0)
                        {
                            int n = input.Read(buf, 0, (int)Math.Min(buf.Length, left));
                            if (n <= 0) throw new Exception("the connection closed during the upload");
                            f.Write(buf, 0, n);
                            left -= n;
                        }
                    }
                }
                catch
                {
                    File.Delete(tmp);
                    throw;
                }
                if (File.Exists(exe))
                {
                    Helper(exe, "stop");
                    // Windows keeps a running executable's file: the old one is renamed away.
                    // Copies left by earlier updates go once nothing runs them any more.
                    foreach (string old in Directory.GetFiles(dir, "marsad-agent.exe.old.*"))
                    {
                        try { File.Delete(old); }
                        catch (UnauthorizedAccessException) { /* still running */ }
                        catch (IOException) { /* still running */ }
                    }
                    File.Move(exe, exe + ".old." + pid);
                }
                File.Move(tmp, exe);
            }
            else if (cmd[0] == "RESTART") Helper(exe, "stop");
            else if (cmd[0] != "RUN") throw new Exception("unknown command " + cmd[0]);
            Say("MARSAD-OK");
            return 0;
        }
        catch (Exception e)
        {
            byte[] b = Encoding.UTF8.GetBytes("Marsad bootstrap: " + e.Message + "\n");
            Stream err = Console.OpenStandardError();
            err.Write(b, 0, b.Length);
            err.Flush();
            return 1;
        }
    }
}
