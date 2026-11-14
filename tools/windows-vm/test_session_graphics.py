from pathlib import Path
import re
import unittest


SCRIPT = Path(__file__).with_name("session-graphics-evidence.ps1")


class SessionGraphicsContractTests(unittest.TestCase):
    def test_collection_has_no_lifecycle_configuration_input_or_device_creation(self):
        source = SCRIPT.read_text() + SCRIPT.with_name("session-identity.ps1").read_text()
        for forbidden in ("Start-Process", "Stop-Process", "Start-ScheduledTask", "Register-ScheduledTask",
                          "Set-Item", "New-ItemProperty", "Set-PnpDevice", "Enable-WinEvent", "logoff",
                          "D3D11", "D3D12", "CreateDevice", "SendInput", "Start-Sleep", "WaitForExit"):
            self.assertNotIn(forbidden.lower(), source.lower())
        methods = re.findall(r"Invoke-CimMethod[^\n]*?-MethodName (\w+)", source)
        self.assertEqual(methods, ["GetOwner", "GetOwnerSid"])
        self.assertIn("-q -d 'PIDS,MEMORY'", source)
        self.assertIn("[IO.File]::Move($pending,$Output)", source)
        self.assertNotIn("[IO.File]::Delete", source)

    def test_context_and_query_failures_are_retained_separately(self):
        source = SCRIPT.read_text()
        for field in ("ExpectedAccount", "ExpectedSessionId", "ExpectedUserSid", "ExpectedLogonSid",
                      "ExpectedBootUtc", "process_start_utc", "user_sid", "logon_sid", "same_logon_verified",
                      "sid_return_value", "sid_query_error", "private_bytes", "scan_capped", "message_error"):
            self.assertIn(field, source)
        helper = SCRIPT.with_name("session-identity.ps1").read_text()
        self.assertIn("Get-CapySessionIdentity", source)
        self.assertNotIn("logon_sids", source)
        self.assertNotIn(".Groups", helper)
        self.assertIn("GetTokenInformation(token, 28,", helper)
        self.assertIn("Marshal.ReadInt32(buffer) != 1", helper)
        self.assertIn("Marshal.ReadIntPtr(buffer, IntPtr.Size)", helper)
        self.assertIn("new SecurityIdentifier(sid).Value", helper)
        self.assertIn("Marshal.FreeHGlobal(buffer)", helper)
        self.assertIn("[Security.Principal.WindowsIdentity]::GetCurrent()", helper)
        self.assertIn("[Diagnostics.Process]::GetCurrentProcess()", helper)
        self.assertIn("if($log.IsEnabled)", source)
        self.assertIn("-MaxEvents 4000", source)


if __name__ == "__main__":
    unittest.main()
