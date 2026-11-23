import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Deploys to GitHub Pages at https://noemaforge.github.io/vimanam/
export default defineConfig({
  site: 'https://noemaforge.github.io',
  base: '/vimanam',
  integrations: [
    starlight({
      title: 'Vimanam',
      description:
        'Turn OpenAPI, Protocol Buffers, and GraphQL contracts into token-budgeted Markdown, agent Skill trees, and deterministic breaking-change diffs.',
      favicon: '/favicon.svg',
      logo: {
        src: './src/assets/vimanam-mark.svg',
        alt: 'Vimanam',
      },
      head: [
        {
          tag: 'link',
          attrs: {
            rel: 'preconnect',
            href: 'https://fonts.googleapis.com',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'preconnect',
            href: 'https://fonts.gstatic.com',
            crossorigin: '',
          },
        },
        {
          tag: 'link',
          attrs: {
            rel: 'stylesheet',
            href: 'https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600&family=Schibsted+Grotesk:ital,wght@0,400..800;1,400..700&display=swap',
          },
        },
        {
          tag: 'meta',
          attrs: {
            property: 'og:type',
            content: 'website',
          },
        },
      ],
      customCss: ['./src/styles/custom.css'],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/noemaforge/vimanam',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/noemaforge/vimanam/edit/main/site/',
      },
      expressiveCode: {
        themes: ['github-dark-default', 'github-light-default'],
        styleOverrides: {
          borderRadius: '0.5rem',
          codeFontFamily:
            "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
        },
      },
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Quickstart', slug: 'quickstart' },
            { label: 'Installation & completions', slug: 'installation' },
          ],
        },
        {
          label: 'LLM context & docs',
          items: [
            { label: 'Token budgets & selection', slug: 'llm-context' },
            { label: 'Split trees & Skill mode', slug: 'split-and-skills' },
          ],
        },
        {
          label: 'Input formats',
          items: [
            { label: 'OpenAPI & Swagger', slug: 'openapi' },
            { label: 'Protocol Buffers & gRPC', slug: 'protobuf' },
            { label: 'GraphQL SDL & introspection', slug: 'graphql' },
          ],
        },
        {
          label: 'Diffing & automation',
          items: [
            { label: 'Contract diff & Git refs', slug: 'diff' },
            { label: 'Neutral JSON contract v2', slug: 'json-contracts' },
            { label: 'CI pipelines & Rust library', slug: 'ci-and-library' },
          ],
        },
        {
          label: 'Reference & benchmarks',
          items: [
            { label: 'CLI reference', slug: 'cli-reference' },
            { label: 'Benchmarks & v2.2 analysis', slug: 'benchmarks' },
          ],
        },
      ],
    }),
  ],
});
