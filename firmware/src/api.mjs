export async function api(path, data, { binary = false } = {}) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 6000);
  try {
    const response = await fetch(path, { cache: 'no-store', signal: controller.signal,
      ...(data === undefined ? {} : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(data) }) });
    if (binary && response.ok && response.headers.get('Content-Type') === 'application/octet-stream') {
      return await response.blob();
    }
    let value;
    try { value = await response.json(); } catch { throw new Error('盒子返回了无法读取的结果，请刷新页面重试'); }
    if (!response.ok || value.ok === false) throw new Error(value.message || '这次操作没有完成，请重试');
    return value;
  } catch (error) {
    if (error.name === 'AbortError' || error instanceof TypeError)
      throw new Error('暂时无法连接盒子，请检查供电和 Wi-Fi，再重新查看当前状态');
    throw error;
  } finally { clearTimeout(timeout); }
}
