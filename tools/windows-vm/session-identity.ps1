function Get-CapySessionIdentity {
    if(!('CapySessionToken' -as [type])){
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
public static class CapySessionToken {
    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetTokenInformation(IntPtr token, int informationClass, IntPtr buffer, uint length, out uint needed);
    public static string LogonSid(IntPtr token) {
        uint needed;
        bool queried = GetTokenInformation(token, 28, IntPtr.Zero, 0, out needed);
        int error = Marshal.GetLastWin32Error();
        if (queried || error != 122) throw new Win32Exception(error, "TokenLogonSid size query failed");
        if (needed < IntPtr.Size * 2 + sizeof(uint)) throw new InvalidOperationException("TokenLogonSid buffer is too small");
        IntPtr buffer = Marshal.AllocHGlobal(checked((int)needed));
        try {
            if (!GetTokenInformation(token, 28, buffer, needed, out needed)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (Marshal.ReadInt32(buffer) != 1) throw new InvalidOperationException("The interactive token has no unique logon SID");
            IntPtr sid = Marshal.ReadIntPtr(buffer, IntPtr.Size);
            if (sid == IntPtr.Zero) throw new InvalidOperationException("TokenLogonSid returned a null SID");
            return new SecurityIdentifier(sid).Value;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
}
'@
    }
    $token=[Security.Principal.WindowsIdentity]::GetCurrent()
    $process=[Diagnostics.Process]::GetCurrentProcess()
    try{
        $logon=[CapySessionToken]::LogonSid($token.Token)
        if($logon -notmatch '^S-1-5-5-\d+-\d+$'){throw 'The interactive token has no logon SID'}
        @{account=$token.Name;user_sid=$token.User.Value;session_id=$process.SessionId;logon_sid=$logon;
            boot_utc=(Get-CimInstance Win32_OperatingSystem -ErrorAction Stop).LastBootUpTime.ToUniversalTime().ToString('o');
            powershell_version=$PSVersionTable.PSVersion.ToString();process_id=$PID;process_start_utc=$process.StartTime.ToUniversalTime().ToString('o')}
    }finally{$process.Dispose();$token.Dispose()}
}
