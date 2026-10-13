import type { WorkspaceFileContent, WorkspaceFileEntry } from 'shared/types';

export type FileViewKind =
  | 'code'
  | 'text'
  | 'markdown'
  | 'image'
  | 'unsupported';

export interface FileDisplayInfo {
  viewKind: FileViewKind;
  language: string | null;
  label: string;
  isPreviewable: boolean;
}

const MARKDOWN_EXTENSIONS = new Set(['md', 'markdown', 'mdx']);

const TEXT_EXTENSIONS = new Set([
  'txt',
  'text',
  'log',
  'lock',
  'csv',
  'tsv',
  'env',
  'gitignore',
  'dockerignore',
  'npmrc',
  'yarnrc',
  'editorconfig',
  'prettierrc',
  'eslintignore',
  'gitattributes',
]);

const CODE_LANGUAGE_BY_EXTENSION: Record<string, string> = {
  ts: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  jsx: 'jsx',
  mjs: 'javascript',
  cjs: 'javascript',
  json: 'json',
  jsonc: 'jsonc',
  rs: 'rust',
  py: 'python',
  go: 'go',
  java: 'java',
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  cc: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  swift: 'swift',
  kt: 'kotlin',
  kts: 'kotlin',
  dart: 'dart',
  rb: 'ruby',
  php: 'php',
  lua: 'lua',
  r: 'r',
  scala: 'scala',
  ex: 'elixir',
  exs: 'elixir',
  html: 'html',
  htm: 'html',
  css: 'css',
  scss: 'scss',
  sass: 'sass',
  less: 'less',
  vue: 'vue',
  svelte: 'svelte',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'toml',
  xml: 'xml',
  sql: 'sql',
  graphql: 'graphql',
  gql: 'graphql',
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  fish: 'fish',
  ps1: 'powershell',
  bat: 'batch',
  cmd: 'batch',
  dockerfile: 'dockerfile',
  makefile: 'makefile',
  cmake: 'cmake',
  proto: 'protobuf',
};

const CODE_FILENAMES: Record<string, string> = {
  dockerfile: 'dockerfile',
  makefile: 'makefile',
  'cargo.lock': 'toml',
  'package-lock.json': 'json',
  'pnpm-lock.yaml': 'yaml',
  'yarn.lock': 'yaml',
  '.eslintrc': 'json',
  '.prettierrc': 'json',
  '.babelrc': 'json',
};

const IMAGE_MIME_PREFIX = 'image/';

export function getWorkspaceFileExtension(pathOrName: string): string {
  const basename = pathOrName.toLowerCase().split('/').pop() ?? '';
  if (!basename.includes('.')) return basename;
  return basename.split('.').pop() ?? '';
}

export function getWorkspaceFileLanguage(
  pathOrName: string,
  backendLanguage?: string | null
): string | null {
  if (backendLanguage) return backendLanguage;

  const basename = pathOrName.toLowerCase().split('/').pop() ?? '';
  const filenameLanguage = CODE_FILENAMES[basename];
  if (filenameLanguage) return filenameLanguage;

  const extension = getWorkspaceFileExtension(pathOrName);
  return CODE_LANGUAGE_BY_EXTENSION[extension] ?? null;
}

export function classifyWorkspaceFile(
  file:
    | Pick<
        WorkspaceFileContent,
        'kind' | 'path' | 'name' | 'mime_type' | 'language'
      >
    | Pick<WorkspaceFileEntry, 'kind' | 'path' | 'name' | 'mime_type'>
): FileDisplayInfo {
  const path = file.path || file.name;
  const extension = getWorkspaceFileExtension(path);
  const mimeType = file.mime_type?.toLowerCase() ?? null;
  const backendLanguage = 'language' in file ? file.language : null;
  const language = getWorkspaceFileLanguage(path, backendLanguage);

  if (file.kind === 'image' || mimeType?.startsWith(IMAGE_MIME_PREFIX)) {
    if (extension === 'svg') {
      return display('code', language ?? 'xml', 'SVG source', true);
    }
    return display('image', null, 'Image', true);
  }

  if (file.kind === 'binary' || file.kind === 'unsupported') {
    return display('unsupported', null, 'Unsupported', false);
  }

  if (MARKDOWN_EXTENSIONS.has(extension) || mimeType === 'text/markdown') {
    return display('markdown', 'markdown', 'Markdown', true);
  }

  if (language) {
    return display('code', language, languageLabel(language), true);
  }

  if (
    file.kind === 'text' ||
    mimeType?.startsWith('text/') ||
    TEXT_EXTENSIONS.has(extension)
  ) {
    return display('text', null, 'Text', true);
  }

  return display('unsupported', null, 'Unsupported', false);
}

export function isWorkspaceFilePreviewable(viewKind: FileViewKind): boolean {
  return viewKind !== 'unsupported';
}

function display(
  viewKind: FileViewKind,
  language: string | null,
  label: string,
  isPreviewable: boolean
): FileDisplayInfo {
  return {
    viewKind,
    language,
    label,
    isPreviewable,
  };
}

function languageLabel(language: string): string {
  switch (language) {
    case 'tsx':
      return 'TSX';
    case 'jsx':
      return 'JSX';
    case 'jsonc':
      return 'JSONC';
    case 'csharp':
      return 'C#';
    case 'cpp':
      return 'C++';
    default:
      return language.charAt(0).toUpperCase() + language.slice(1);
  }
}
