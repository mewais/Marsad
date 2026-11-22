using System;using System.IO;using System.Text;using System.Runtime.InteropServices;using Microsoft.Win32.SafeHandles;
static class Probe{
[DllImport("kernel32")]static extern IntPtr GetStdHandle(int n);
[DllImport("kernel32")]static extern int GetFileType(IntPtr h);
[DllImport("ntdll")]static extern int NtQueryInformationFile(IntPtr h,long[] iosb,out int info,int len,int cls);
static Stream O=Console.OpenStandardOutput();
static void Say(string t){var x=Encoding.ASCII.GetBytes(t+"\n");O.Write(x,0,x.Length);O.Flush();}
static int Main(string[] a){
IntPtr h=GetStdHandle(-10);int mode;int st=NtQueryInformationFile(h,new long[2],out mode,4,16);
Console.Error.WriteLine("stdin: type "+GetFileType(h)+" mode 0x"+mode.ToString("x")+" (status 0x"+st.ToString("x")+"; 0x10/0x20 = synchronous)");
Stream I;
if(a[0]=="sync")I=Console.OpenStandardInput();
else{try{I=new FileStream(new SafeFileHandle(h,false),FileAccess.Read,1,true);Console.Error.WriteLine("async FileStream");}
catch(Exception e){Console.Error.WriteLine("async refused: "+e.Message);I=new FileStream(new SafeFileHandle(h,false),FileAccess.Read,1,false);}}
Say("PROBE-READY");
var s=new StringBuilder();for(;;){int c=I.ReadByte();if(c<0){Say("PROBE-EOF");return 1;}if(c==10)break;s.Append((char)c);}
Say("PROBE-GOT "+s);
var p=s.ToString().Split(' ');long n=long.Parse(p[1]);var buf=new byte[65536];long tot=0;
while(tot<n){int r=I.Read(buf,0,(int)Math.Min(buf.Length,n-tot));if(r<=0){Say("PROBE-EOF at "+tot);return 1;}tot+=r;}
Say("PROBE-DONE "+tot);return 0;}}
