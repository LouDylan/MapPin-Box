export function hotspotChange(ssid, password = '') {
  if (typeof ssid !== 'string' || !ssid.trim() || new TextEncoder().encode(ssid).length > 32 || /[\u0000-\u001f\u007f-\u009f]/u.test(ssid))
    throw new Error('热点名称需要1至32字节，不能全为空格或包含控制字符。');
  if (typeof password !== 'string' || (password && (password.length < 8 || password.length > 63 || /[^\x20-\x7e]/u.test(password))))
    throw new Error('热点密码需要8至63位英文、数字或符号，留空保持原密码。');
  return { ssid, password: password || null };
}
