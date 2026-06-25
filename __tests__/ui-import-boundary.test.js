import { readdirSync, readFileSync, statSync } from 'node:fs';
import { extname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const UI_DIR = fileURLToPath(new URL('../ui/', import.meta.url));

function listJsFiles(dir) {
    const out = [];
    for (const name of readdirSync(dir)) {
        const full = join(dir, name);
        if (statSync(full).isDirectory()) out.push(...listJsFiles(full));
        else if (extname(name) === '.js') out.push(full);
    }
    return out;
}

describe('ui/ import boundary contract', () => {
    it('no ui/*.js imports a path that escapes ui/', () => {
        const violations = [];
        const re = /\bfrom\s+['"](\.\.[\\/]|\/)/g;
        for (const file of listJsFiles(UI_DIR)) {
            const src = readFileSync(file, 'utf8');
            for (const m of src.matchAll(re))
                violations.push(`${file}: ${m[0]}`);
        }
        expect(violations).toEqual([]);
    });
});
