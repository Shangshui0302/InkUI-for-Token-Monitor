const networks = ['wifi', 'ethernet', 'cellular', 'offline', 'other', 'unknown'];
export function normalizeDeviceStatus(value) {
  const v = value && typeof value === 'object' ? value : {};
  return {
    fullscreen: v.fullscreen === true,
    time: typeof v.time === 'string' && /^(?:[01]\d|2[0-3]):[0-5]\d$/.test(v.time) ? v.time : '—:—',
    batteryPercent: typeof v.batteryPercent === 'number' && Number.isFinite(v.batteryPercent) && v.batteryPercent >= 0 && v.batteryPercent <= 100 ? Math.round(v.batteryPercent) : null,
    charging: typeof v.charging === 'boolean' ? v.charging : null,
    network: networks.includes(v.network) ? v.network : 'unknown',
    networkValidated: typeof v.networkValidated === 'boolean' ? v.networkValidated : null
  };
}
export function networkLabel(status) {
  const name = { wifi:'Wi-Fi', ethernet:'有线网络', cellular:'移动网络', offline:'设备未联网', other:'其他网络', unknown:'网络状态未知' }[status.network];
  return status.networkValidated === false && !['offline','unknown'].includes(status.network) ? name + ' · 外网未验证，局域网仍可能可用' : name;
}
