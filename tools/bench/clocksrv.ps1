# UDP time echo server (same protocol as clock_offset.py serve): reply = request + big-endian
# float64 epoch milliseconds. Lets a viewer machine measure this host's clock offset.
param([int]$Port = 47999)
$udp = New-Object System.Net.Sockets.UdpClient($Port)
$ep = New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Any, 0)
while ($true) {
  $data = $udp.Receive([ref]$ep)
  $ms = [double]([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()) + ([DateTime]::UtcNow.Ticks % 10000) / 10000.0
  $t = [BitConverter]::GetBytes($ms)
  [Array]::Reverse($t)  # to network byte order
  $reply = New-Object byte[] ($data.Length + 8)
  [Array]::Copy($data, $reply, $data.Length)
  [Array]::Copy($t, 0, $reply, $data.Length, 8)
  [void]$udp.Send($reply, $reply.Length, $ep)
}
