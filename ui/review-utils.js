export function formatInjectionError(payload) {
    const raw = typeof payload === 'string' ? payload.slice(0, 500) : '';
    // Backend now sends a complete user-facing Chinese sentence — show it
    // as-is; a "粘贴失败：" prefix would duplicate it.
    return raw || '粘贴失败，剪贴板被占用';
}
