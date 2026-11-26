/// The Markdown the editor understands, as worked examples.
///
/// Each rule's `source` is rendered on the page by the same `renderMarkdown`
/// the editor's preview uses, so the result beside it is what a post gets, not
/// a description of it. Only what `@ign1s-reiga/marked-presets` actually turns
/// on belongs here: GFM with `breaks: true`, alerts, footnotes, heading anchors,
/// and Shiki with the diff / highlight / word / error-level notations.

export type Rule = {
  id: string;
  title: string;
  /// One or two sentences. Text in backticks is set as code.
  note?: string;
  source: string;
};

export type Section = { id: string; title: string; rules: Rule[] };

/// Languages that get syntax highlighting, by the exact name to write after the
/// fence. Mirrors `LANG_NAMES` in marked-presets, which does not export it; the
/// renderer matches these names exactly, so `rs` is highlighted and `rust` is
/// plain text. rules.test.ts fails if the two drift apart.
export const LANGUAGES: [id: string, name: string][] = [
  ['js', 'JavaScript'],
  ['jsx', 'JSX'],
  ['ts', 'TypeScript'],
  ['tsx', 'TSX'],
  ['html', 'HTML'],
  ['css', 'CSS'],
  ['json', 'JSON'],
  ['toml', 'TOML'],
  ['yaml', 'YAML'],
  ['dotenv', '.env'],
  ['rs', 'Rust'],
  ['cs', 'C#'],
  ['cpp', 'C++'],
  ['java', 'Java'],
  ['kt', 'Kotlin'],
  ['py', 'Python'],
  ['shell', 'Shell'],
  ['powershell', 'PowerShell'],
  ['sql', 'SQL'],
  ['md', 'Markdown'],
  ['mdx', 'MDX'],
  ['diff', 'Diff'],
  ['nix', 'Nix'],
  ['dockerfile', 'Dockerfile'],
];

/// The image path in the Images example, written the way the editor inserts a
/// post's media. The file ships with the app at `public/assets/harbour.webp`;
/// the page points the rendered `<img>` at it from the site root, so it does
/// not depend on which route the page is served under.
export const SAMPLE_IMAGE = 'assets/harbour.webp';

const md = (...lines: string[]) => lines.join('\n');

export const SECTIONS: Section[] = [
  {
    id: 'text',
    title: 'Text',
    rules: [
      {
        id: 'paragraphs',
        title: 'Paragraphs and line breaks',
        note: 'A line break in the source is a line break in the post. Leave a blank line to start a new paragraph.',
        source: md('First line', 'Second line, directly below it', '', 'A blank line above starts a new paragraph.'),
      },
      {
        id: 'emphasis',
        title: 'Emphasis',
        note: '`*` and `_` both work for bold and italic.',
        source: md(
          '**Bold**, _italic_, ***bold italic*** and ~~struck through~~.',
          '',
          '**This text is _extremely_ important.**',
        ),
      },
      {
        id: 'inline-code',
        title: 'Inline code',
        source: md('Run `pnpm install`, then open `.env` and set `PORT=3000`.'),
      },
    ],
  },
  {
    id: 'headings',
    title: 'Headings',
    rules: [
      {
        id: 'headings',
        title: 'Headings',
        note: '`##` to `####` get a link to their own section. The post title is its own field, so a post usually starts at `##`.',
        source: md('## Section', '', '### Subsection with `code`', '', '#### Detail', '', 'Text under the detail.'),
      },
    ],
  },
  {
    id: 'links',
    title: 'Links and images',
    rules: [
      {
        id: 'links',
        title: 'Links',
        note: 'A bare address links itself.',
        source: md('Read the [marked docs](https://marked.js.org).', '', 'Or go straight to https://shiki.style'),
      },
      {
        id: 'images',
        title: 'Images and captions',
        note: 'Italic text on the line right after an image becomes its caption. Insert images from Media or by dropping a file on the editor; they go in as `assets/…` paths.',
        source: md(`![Harbour at dusk](${SAMPLE_IMAGE})`, '*The harbour, an hour after sunset.*'),
      },
    ],
  },
  {
    id: 'lists',
    title: 'Lists',
    rules: [
      {
        id: 'bullets',
        title: 'Bulleted lists',
        note: 'Indent by two spaces to nest.',
        source: md('- First item', '- Second item', '  - Nested item', '  - Another nested item', '- Third item'),
      },
      {
        id: 'numbered',
        title: 'Numbered lists',
        note: 'Indent by three spaces to nest. The first number sets where the list starts.',
        source: md('1. First', '2. Second', '   1. Nested', '   2. Nested again', '3. Third'),
      },
      {
        id: 'tasks',
        title: 'Task lists',
        note: 'The boxes show the state; readers cannot tick them.',
        source: md('- [x] Pick a title', '- [ ] Draft the outline', '- [ ] Add a thumbnail'),
      },
    ],
  },
  {
    id: 'quotes',
    title: 'Quotes and alerts',
    rules: [
      {
        id: 'blockquote',
        title: 'Quotes',
        note: 'A line with only `>` keeps the quote going across a blank line. Add another `>` to nest.',
        source: md(
          '> A quoted paragraph.',
          '>',
          '> A second paragraph in the same quote.',
          '>',
          '> > A quote inside it.',
        ),
      },
      {
        id: 'alerts',
        title: 'Alerts',
        note: 'A quote whose first line is `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]`.',
        source: md(
          '> [!NOTE]',
          '> Useful information, even when skimming.',
          '',
          '> [!TIP]',
          '> Advice for doing it better.',
          '',
          '> [!IMPORTANT]',
          '> Something the reader needs to succeed.',
          '',
          '> [!WARNING]',
          '> Needs attention right away.',
          '',
          '> [!CAUTION]',
          '> What could go wrong if they do this.',
        ),
      },
    ],
  },
  {
    id: 'code',
    title: 'Code',
    rules: [
      {
        id: 'fenced',
        title: 'Code blocks',
        note: 'Name the language right after the opening fence, using a name from the list above. Readers get a Copy button.',
        source: md('```ts', 'export function add(a: number, b: number): number {', '  return a + b;', '}', '```'),
      },
      {
        id: 'plain',
        title: 'Plain text',
        note: 'A block with no language, or one not in the list, is shown as plain text.',
        source: md('```', 'GET /posts 200 12ms', 'GET /tags  200  3ms', '```'),
      },
      {
        id: 'diff-language',
        title: 'Diffs',
        source: md('```diff', "- gem 'rubocop', require: false", "+ gem 'rubocop'", '```'),
      },
      {
        id: 'mark-diff',
        title: 'Mark lines as added or removed',
        note: "End the line with a comment holding `[!code ++]` or `[!code --]`, in that language's comment syntax. The marker is removed from the output.",
        source: md(
          '```js',
          'export function greet() {',
          "  console.log('hewwo'); // [!code --]",
          "  console.log('hello'); // [!code ++]",
          '}',
          '```',
        ),
      },
      {
        id: 'mark-highlight',
        title: 'Highlight a line',
        source: md('```py', 'def total(items):', '    return sum(i.price for i in items)  # [!code highlight]', '```'),
      },
      {
        id: 'mark-word',
        title: 'Highlight a word',
        note: 'A comment line with `[!code word:Hello]` highlights that word in the lines after it. `[!code word:Hello:2]` stops after two.',
        source: md(
          '```js',
          '// [!code word:Hello]',
          "const msg = 'Hello World';",
          'console.log(msg); // prints Hello World',
          '```',
        ),
      },
      {
        id: 'mark-level',
        title: 'Mark errors and warnings',
        source: md(
          '```js',
          'export function check() {',
          "  console.error('Failed'); // [!code error]",
          "  console.warn('Slow'); // [!code warning]",
          '}',
          '```',
        ),
      },
    ],
  },
  {
    id: 'tables',
    title: 'Tables',
    rules: [
      {
        id: 'tables',
        title: 'Tables',
        note: 'Colons in the divider row set alignment: `:--` left, `--:` right, `:-:` centre. The columns do not need to line up in the source.',
        source: md(
          '| Feature | Status | Notes |',
          '| :-- | :-: | --: |',
          '| Alerts | ✅ | 5 kinds |',
          '| Footnotes | ✅ | at the end |',
        ),
      },
    ],
  },
  {
    id: 'footnotes',
    title: 'Footnotes',
    rules: [
      {
        id: 'footnotes',
        title: 'Footnotes',
        note: 'The label can be a number or a word. Notes are numbered in the order they are referenced and listed at the end of the post, wherever you write them.',
        source: md(
          'Marked is a fast Markdown parser.[^1] It takes extensions.[^ext]',
          '',
          '[^1]: [marked on GitHub](https://github.com/markedjs/marked)',
          '[^ext]: Alerts and footnotes are two of them.',
        ),
      },
    ],
  },
  {
    id: 'dividers',
    title: 'Dividers',
    rules: [
      {
        id: 'divider',
        title: 'Horizontal rule',
        note: 'Leave a blank line above `---`. Directly under a line of text, it makes that line a heading instead.',
        source: md('Above the line.', '', '---', '', 'Below it.'),
      },
    ],
  },
];
