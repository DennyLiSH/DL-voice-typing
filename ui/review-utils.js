import { INJECTION_ERROR_MSG } from './lib/errors.js';

export function formatInjectionError(payload) {
    const raw = typeof payload === 'string' ? payload.slice(0, 500) : '';
    // Backend now sends a complete user-facing Chinese sentence — show it
    // as-is; a "粘贴失败：" prefix would duplicate it.
    return raw || INJECTION_ERROR_MSG;
}
