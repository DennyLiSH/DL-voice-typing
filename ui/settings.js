const { invoke } = window.__TAURI__.core;

const apiUrlInput = document.getElementById('api-url');
const apiKeyInput = document.getElementById('api-key');
const modelInput = document.getElementById('model');
const testBtn = document.getElementById('test-btn');
const saveBtn = document.getElementById('save-btn');
const statusEl = document.getElementById('status');
const toggleKeyBtn = document.getElementById('toggle-key');

// Load current config
async function loadConfig() {
    try {
        const config = await invoke('get_config');
        apiUrlInput.value = config.llm_api_url || '';
        apiKeyInput.value = config.llm_api_key || '';
        modelInput.value = config.llm_model || '';
    } catch (e) {
        console.error('Failed to load config:', e);
    }
}

// Toggle API key visibility
toggleKeyBtn.addEventListener('click', () => {
    const input = apiKeyInput;
    if (input.type === 'password') {
        input.type = 'text';
        toggleKeyBtn.textContent = '🔒';
    } else {
        input.type = 'password';
        toggleKeyBtn.textContent = '👁';
    }
});

function setStatus(message, type) {
    statusEl.textContent = message;
    statusEl.className = 'status ' + type;
}

// Test connection
testBtn.addEventListener('click', async () => {
    const apiUrl = apiUrlInput.value.trim();
    const apiKey = apiKeyInput.value.trim();
    const model = modelInput.value.trim();

    if (!apiUrl || !apiKey || !model) {
        setStatus('请填写所有字段', 'error');
        return;
    }

    testBtn.disabled = true;
    testBtn.textContent = '测试中...';
    statusEl.textContent = '';

    try {
        await invoke('test_llm_connection', { apiUrl, apiKey, model });
        setStatus('✓ 连接成功', 'success');
    } catch (e) {
        setStatus('✗ 连接失败: ' + e, 'error');
    } finally {
        testBtn.disabled = false;
        testBtn.textContent = '测试连接';
    }
});

// Save settings
saveBtn.addEventListener('click', async () => {
    const apiUrl = apiUrlInput.value.trim();
    const apiKey = apiKeyInput.value.trim();
    const model = modelInput.value.trim();

    try {
        await invoke('save_llm_settings', { apiUrl, apiKey, model });
        setStatus('✓ 设置已保存', 'success');
    } catch (e) {
        setStatus('✗ 保存失败: ' + e, 'error');
    }
});

loadConfig();
