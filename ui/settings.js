// Settings page orchestrator.
//
// Responsibility is split across focused modules:
//   - app-shell.js: sidebar routing, global error banner, version, init
//   - settings-form.js: form fields, toggles, dirty state, save, test connection
//   - model-manager.js: Whisper model select, download, compute mode badge
//   - data-manager.js: saved recordings list, search, playback, delete
//
// Importing the modules registers their event listeners; init() kicks off
// configuration loading and initial UI population.

import { init } from './app-shell.js';
import './settings-form.js';
import './model-manager.js';
import './data-manager.js';

init();
