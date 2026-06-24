export function formatInjectionError(payload) {
    const raw = typeof payload === 'string' ? payload.slice(0, 500) : '';
    return raw ? `粘贴失败：${raw}` : '粘贴失败，剪贴板被占用';
}
