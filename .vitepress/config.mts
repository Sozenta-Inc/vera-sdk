import {defineConfig} from 'vitepress';
import {readdirSync} from 'fs';
import {dirname, join} from 'path';
import {fileURLToPath} from 'url';

// Auto-discover the guides at the repo root (same pattern as Veya's
// docs site). Add a GUIDE-*.md / FORMAT-*.md → it appears in the
// sidebar on the next build, no config edit.

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');

function list(prefix: string): {text: string; link: string}[] {
  try {
    return readdirSync(ROOT)
      .filter((f) => f.endsWith('.md') && f.startsWith(prefix))
      .sort()
      .map((f) => ({
        text: f.replace(/\.md$/, '').replace(`${prefix}-`, '').replace(/-/g, ' '),
        link: `/${f.replace(/\.md$/, '')}`,
      }));
  } catch {
    return [];
  }
}

export default defineConfig({
  title: 'Vera',
  description: 'Sovereign AI gateway — SDK, guides, and wire formats',
  // Public project Pages serves at https://<org>.github.io/vera-sdk/.
  base: '/vera-sdk/',
  cleanUrls: true,
  lastUpdated: true,
  ignoreDeadLinks: true,
  srcExclude: ['sdk/**', 'demos/**', 'node_modules/**'],
  rewrites: {'README.md': 'index.md'},
  // Placeholder syntax like <your-vera> in plain text breaks Vue's
  // template compiler with html:true — we never use inline HTML anyway.
  markdown: {html: false},
  themeConfig: {
    nav: [
      {text: 'Guides', link: '/GUIDE-METAL'},
      {text: 'Formats', link: '/FORMAT-bot-bundle'},
      {text: 'Gateway repo', link: 'https://github.com/Sozenta-Inc/vera'},
    ],
    sidebar: [
      {text: 'Overview', items: [{text: 'Vera SDK', link: '/'}]},
      {text: 'Guides', items: list('GUIDE')},
      {text: 'Wire formats', items: list('FORMAT')},
    ],
    search: {provider: 'local'},
    socialLinks: [{icon: 'github', link: 'https://github.com/Sozenta-Inc/vera-sdk'}],
  },
});
