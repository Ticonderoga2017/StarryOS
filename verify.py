#!/usr/bin/env python3  
"""  
验证 StarryOS WPA2 四次握手 M2 帧的正确性  
数据来源：同一次运行的日志  
"""  
import hmac  
import hashlib  
import struct  
  
# ========== 从日志中提取的数据 ==========  
  
# PMK (已验证与 wpa_passphrase 一致)  
pmk = bytes.fromhex(  
    "eaf4321a66940a1c1fb36c6e43090eb29c9cdf1a47b6637c69f13f1ac9d23c6c"  
)  
  
# AA (AP BSSID) 和 SPA (STA MAC)  
aa  = bytes.fromhex("8c83e8265908")  
spa = bytes.fromhex("387acc942d2c")  
  
# ANonce (从 M1 bytes[17:49])  
anonce = bytes.fromhex(  
    "0af294b84fa8bca7498739 87ebb552257dec265082 0d0e1d1c1ac1b830f9f50f"  
    .replace(" ", "")  
)  
  
# SNonce (从日志 wpa2 SNonce 行)  
snonce = bytes.fromhex(  
    "fb7260f5c026caab1c4fd45d0623f6ec6845bbcf8be5d23b75b98cc09801cf93"  
)  
  
# StarryOS 计算的 PTK  
starry_kck = bytes.fromhex("59ee15e650216af38c349df2c78b9935")  
starry_kek = bytes.fromhex("a4c1928f3248a1a5282277ff662bceb5")  
starry_tk  = bytes.fromhex("453b484ef0cf89b5487adb0a4884d465")  
  
# 第一个 M2 完整帧 (121 bytes, 含 MIC)  
m2_hex = (  
    "01 03 00 75 02 01 0a 00 00 00 00 00 00 00 00 00"  
    " 01 fb 72 60 f5 c0 26 ca ab 1c 4f d4 5d 06 23 f6"  
    " ec 68 45 bb cf 8b e5 d2 3b 75 b9 8c c0 98 01 cf"  
    " 93 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"  
    " 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"  
    " 5c 17 24 dd 97 cd 4f db c3 85 4f 17 47 4c 9b 8e"  
    " 00 16 30 14 01 00 00 0f ac 04 01 00 00 0f ac 04"  
    " 01 00 00 0f ac 02 0c 00"  
)  
m2 = bytes.fromhex(m2_hex.replace(" ", ""))  
  
# ========== PRF-SHA1 实现 ==========  
  
def hmac_sha1(key: bytes, data: bytes) -> bytes:  
    return hmac.new(key, data, hashlib.sha1).digest()  
  
def prf_sha1(key: bytes, label: bytes, data: bytes, output_len: int) -> bytes:  
    """IEEE 802.11i PRF-X"""  
    n_iter = (output_len + 19) // 20  # ceil(output_len / 20)  
    result = b""  
    for i in range(n_iter):  
        msg = label + b"\x00" + data + bytes([i])  
        result += hmac_sha1(key, msg)  
    return result[:output_len]  
  
# ========== PTK 推导 ==========  
  
# 地址排序  
min_addr = min(aa, spa)  
max_addr = max(aa, spa)  
  
# Nonce 排序  
min_nonce = min(anonce, snonce)  
max_nonce = max(anonce, snonce)  
  
print("=" * 60)  
print("地址排序验证")  
print("=" * 60)  
print(f"  AA:  {aa.hex()}")  
print(f"  SPA: {spa.hex()}")  
print(f"  min: {min_addr.hex()} ({'SPA' if min_addr == spa else 'AA'})")  
print(f"  max: {max_addr.hex()} ({'AA' if max_addr == aa else 'SPA'})")  
  
print(f"\nNonce 排序验证")  
print(f"  ANonce: {anonce[:4].hex()}...")  
print(f"  SNonce: {snonce[:4].hex()}...")  
print(f"  min: {'ANonce' if min_nonce == anonce else 'SNonce'} ({min_nonce[:4].hex()}...)")  
print(f"  max: {'SNonce' if max_nonce == snonce else 'ANonce'} ({max_nonce[:4].hex()}...)")  
  
# 构造 PRF 输入 data  
prf_data = min_addr + max_addr + min_nonce + max_nonce  
assert len(prf_data) == 76, f"PRF data length should be 76, got {len(prf_data)}"  
  
# 推导 PTK (384 bits = 48 bytes)  
ptk = prf_sha1(pmk, b"Pairwise key expansion", prf_data, 48)  
py_kck = ptk[0:16]  
py_kek = ptk[16:32]  
py_tk  = ptk[32:48]  
  
print("\n" + "=" * 60)  
print("PTK 推导验证")  
print("=" * 60)  
print(f"  Python KCK:  {py_kck.hex()}")  
print(f"  Starry KCK:  {starry_kck.hex()}")  
print(f"  KCK match:   {py_kck == starry_kck}")  
  
print(f"\n  Python KEK:  {py_kek.hex()}")  
print(f"  Starry KEK:  {starry_kek.hex()}")  
print(f"  KEK match:   {py_kek == starry_kek}")  
  
print(f"\n  Python TK:   {py_tk.hex()}")  
print(f"  Starry TK:   {starry_tk.hex()}")  
print(f"  TK match:    {py_tk == starry_tk}")  
  
# ========== MIC 验证 ==========  
  
# 将 M2 的 MIC 字段清零后计算 MIC  
m2_zeroed = bytearray(m2)  
m2_zeroed[81:97] = b"\x00" * 16  
  
py_mic = hmac_sha1(py_kck, bytes(m2_zeroed))[:16]  
m2_mic = m2[81:97]  
  
print("\n" + "=" * 60)  
print("MIC 验证")  
print("=" * 60)  
print(f"  Python MIC:  {py_mic.hex()}")  
print(f"  M2 MIC:      {m2_mic.hex()}")  
print(f"  MIC match:   {py_mic == m2_mic}")  
  
# ========== M2 帧结构验证 ==========  
  
print("\n" + "=" * 60)  
print("M2 帧结构验证")  
print("=" * 60)  
print(f"  总长度:       {len(m2)} (期望 121)")  
print(f"  Version:      0x{m2[0]:02x} (期望 0x01)")  
print(f"  Type:         0x{m2[1]:02x} (期望 0x03 = EAPOL-Key)")  
print(f"  Body Length:  {(m2[2]<<8)|m2[3]} (期望 117)")  
print(f"  Desc Type:    0x{m2[4]:02x} (期望 0x02 = RSN)")  
print(f"  Key Info:     0x{(m2[5]<<8)|m2[6]:04x} (期望 0x010a)")  
print(f"  Key Length:   {(m2[7]<<8)|m2[8]} (期望 0)")  
print(f"  Replay Ctr:   {m2[9:17].hex()} (期望与 M1 一致)")  
print(f"  SNonce:       {m2[17:49].hex()}")  
print(f"  Key IV:       {m2[49:65].hex()} (期望全零)")  
print(f"  Key RSC:      {m2[65:73].hex()} (期望全零)")  
print(f"  Reserved:     {m2[73:81].hex()} (期望全零)")  
print(f"  MIC:          {m2[81:97].hex()}")  
print(f"  Key Data Len: {(m2[97]<<8)|m2[98]} (期望 22)")  
print(f"  Key Data:     {m2[99:121].hex()}")  
  
# RSN IE 验证  
rsn_ie = m2[99:121]  
assoc_rsn = bytes.fromhex("3014010000 0fac040100 000fac0401 00000fac02 0c00".replace(" ", ""))  
print(f"\n  RSN IE match AssocReq: {rsn_ie == assoc_rsn}")  
  
# ========== PRF-512 验证 (检查 byte 52 bug) ==========  
  
print("\n" + "=" * 60)  
print("PRF-512 测试 (检查 byte 52)")  
print("=" * 60)  
prf512_key = bytes([0x0b] * 20)  
prf512_result = prf_sha1(prf512_key, b"prefix", b"Hi There", 64)  
print(f"  PRF-512 前 16 字节: {prf512_result[:16].hex()}")  
print(f"  PRF-512 byte 52:    0x{prf512_result[52]:02x}")  
print(f"  PRF-512 全部 64 字节:")  
for i in range(0, 64, 16):  
    print(f"    [{i:2d}..{i+15:2d}]: {prf512_result[i:i+16].hex()}")  
  
# ========== 用 StarryOS 的 KCK 计算 MIC ==========  
  
print("\n" + "=" * 60)  
print("用 StarryOS KCK 计算 MIC (排除 PTK 推导问题)")  
print("=" * 60)  
starry_mic = hmac_sha1(starry_kck, bytes(m2_zeroed))[:16]  
print(f"  Starry KCK MIC: {starry_mic.hex()}")  
print(f"  M2 MIC:         {m2_mic.hex()}")  
print(f"  Match:           {starry_mic == m2_mic}")  
  
# ========== 总结 ==========  
  
print("\n" + "=" * 60)  
print("总结")  
print("=" * 60)  
all_pass = True  
checks = [  
    ("PMK", pmk.hex() == "eaf4321a66940a1c1fb36c6e43090eb29c9cdf1a47b6637c69f13f1ac9d23c6c"),  
    ("KCK match", py_kck == starry_kck),  
    ("KEK match", py_kek == starry_kek),  
    ("TK match", py_tk == starry_tk),  
    ("MIC match (Python KCK)", py_mic == m2_mic),  
    ("MIC match (Starry KCK)", starry_mic == m2_mic),  
    ("M2 length", len(m2) == 121),  
    ("M2 version", m2[0] == 0x01),  
    ("M2 body_len", (m2[2]<<8)|m2[3] == 117),  
    ("M2 key_info", (m2[5]<<8)|m2[6] == 0x010a),  
    ("M2 replay=1", m2[9:17] == b"\x00"*7 + b"\x01"),  
    ("RSN IE match", rsn_ie == assoc_rsn),  
]  
for name, result in checks:  
    status = "PASS" if result else "FAIL"  
    if not result:  
        all_pass = False  
    print(f"  [{status}] {name}")  
  
print(f"\n  ALL: {'PASS' if all_pass else 'FAIL'}")  
  
if not all_pass:  
    print("\n  诊断建议:")  
    if py_kck != starry_kck:  
        print("  -> KCK 不匹配: PRF-SHA1 实现有 bug")  
        print("     对比 PRF 的逐次迭代输出来定位问题")  
    if py_mic != m2_mic and py_kck == starry_kck:  
        print("  -> KCK 匹配但 MIC 不匹配: compute_mic 实现有 bug")  
    if starry_mic != m2_mic:  
        print("  -> 用 StarryOS 自己的 KCK 算出的 MIC 也不匹配 M2 中的 MIC")  
        print("     说明 MIC 计算或 M2 帧构造有 bug (MIC_OFFSET 可能错误)")