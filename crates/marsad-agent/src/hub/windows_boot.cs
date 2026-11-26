// The Windows bootstrap's install step (see transport.rs: WINDOWS_BOOT_CS). C# 5, compiled on
// the host by Windows PowerShell's Add-Type (the .NET Framework compiler every Windows has).
// Argument: the build's helper directory. Reports, receives UPLOAD <n> <sha256>, installs.
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using Microsoft.Win32.SafeHandles;

static class MarsadBoot
{
    [DllImport("kernel32.dll")]
    static extern IntPtr GetStdHandle(int which);

    [DllImport("ntdll.dll")]
    static extern int NtQueryInformationFile(IntPtr file, long[] status, out int info, int length, int infoClass);

    static Stream input, output = Console.OpenStandardOutput();

    // OpenSSH for Windows hands a command a pipe opened for overlapped I/O; a FileStream opened
    // as asynchronous reads it properly (a synchronous handle is read synchronously).
    // Unbuffered: the helper started after this program gets every byte it did not read.
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

    static string Esc(string v)
    {
        return v.Replace("%", "%25").Replace(" ", "%20").Replace("=", "%3D").Replace("\r", "%0D").Replace("\n", "%0A");
    }

    static string ReadLine()
    {
        var s = new StringBuilder();
        for (;;)
        {
            int c = input.ReadByte();
            if (c < 0) throw new Exception("the connection closed during the bootstrap");
            if (c == '\n') return s.ToString().TrimEnd('\r');
            s.Append((char)c);
        }
    }

    static int Main(string[] args)
    {
        string tmp = null;
        try
        {
            input = OpenInput();
            string dir = Path.GetFullPath(args[0]);
            string exe = Path.Combine(dir, "marsad-agent.exe");
            Say("MARSAD-BOOT2 os=Windows arch=" + Esc(Environment.GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")) + " helper_build=none daemon=unknown hasher=sha256");
            string[] cmd = ReadLine().Split(' ');
            if (cmd.Length != 3 || cmd[0] != "UPLOAD") throw new Exception("unexpected command " + cmd[0]);
            long total = long.Parse(cmd[1]);
            long left = total;
            Directory.CreateDirectory(dir);
            tmp = exe + ".new." + System.Diagnostics.Process.GetCurrentProcess().Id;
            byte[] hash;
            using (var f = File.Create(tmp))
            using (var sha = SHA256.Create())
            {
                var buf = new byte[65536];
                while (left > 0)
                {
                    int n = input.Read(buf, 0, (int)Math.Min(buf.Length, left));
                    if (n <= 0) throw new Exception("received " + (total - left) + " of " + total + " bytes of the helper");
                    f.Write(buf, 0, n);
                    sha.TransformBlock(buf, 0, n, null, 0);
                    left -= n;
                }
                sha.TransformFinalBlock(buf, 0, 0);
                hash = sha.Hash;
            }
            string got = BitConverter.ToString(hash).Replace("-", "").ToLowerInvariant();
            if (got != cmd[2]) throw new Exception("checksum mismatch (received " + got + ")");
            // Another connection may have installed the same build meanwhile: its file stays.
            if (File.Exists(exe)) File.Delete(tmp); else File.Move(tmp, exe);
            return 0;
        }
        catch (Exception e)
        {
            if (tmp != null) { try { File.Delete(tmp); } catch (Exception) { } }
            // A directory or file the account may not write is a fact of the host, named by its
            // type (the hub explains it); any other failure keeps its own words.
            string reason = e is UnauthorizedAccessException ? "not_writable" : Esc(e.Message);
            Say("MARSAD-FAIL stage=install reason=" + reason);
            byte[] b = Encoding.UTF8.GetBytes("Marsad bootstrap: " + e.Message + "\n");
            Stream err = Console.OpenStandardError();
            err.Write(b, 0, b.Length);
            err.Flush();
            return 1;
        }
    }
}
