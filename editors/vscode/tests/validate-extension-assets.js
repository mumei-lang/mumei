const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const readJson = (relativePath) =>
  JSON.parse(fs.readFileSync(path.join(root, relativePath), 'utf8'));

const manifest = readJson('package.json');
assert.equal(manifest.publisher, 'mumei-lang');
assert.equal(manifest.name, 'mumei');
assert.equal(manifest.version, '0.3.0');
assert.equal(manifest.main, './out/extension.js');
assert.ok(manifest.icon && fs.existsSync(path.join(root, manifest.icon)));
assert.deepEqual(manifest.activationEvents, ['onLanguage:mumei']);
assert.ok(manifest.categories.includes('Programming Languages'));
assert.ok(manifest.categories.includes('Linters'));

const [language] = manifest.contributes.languages;
assert.equal(language.id, 'mumei');
assert.ok(language.extensions.includes('.mm'));
assert.equal(language.configuration, './language-configuration.json');

const [grammarContribution] = manifest.contributes.grammars;
assert.equal(grammarContribution.language, 'mumei');
assert.equal(grammarContribution.scopeName, 'source.mumei');
assert.equal(grammarContribution.path, './syntaxes/mumei.tmLanguage.json');

const config = readJson('language-configuration.json');
assert.deepEqual(config.comments.lineComment, '//');
assert.deepEqual(config.comments.blockComment, ['/*', '*/']);
assert.ok(config.brackets.some(([open, close]) => open === '{' && close === '}'));
assert.ok(
  config.autoClosingPairs.some((pair) => pair.open === '"' && pair.close === '"')
);
assert.match(config.wordPattern, /a-zA-Z_/);

const grammar = readJson('syntaxes/mumei.tmLanguage.json');
assert.equal(grammar.scopeName, 'source.mumei');
assert.ok(grammar.patterns.some((pattern) => pattern.include === '#atom-definition'));
assert.ok(grammar.patterns.some((pattern) => pattern.include === '#operators'));

const repositories = grammar.repository;
assert.match(repositories['control-keywords'].patterns[0].match, /requires/);
assert.match(repositories['control-keywords'].patterns[0].match, /ensures/);
assert.match(repositories['declaration-keywords'].patterns[0].match, /trusted/);
assert.match(repositories['builtin-types'].patterns[0].match, /Str/);
assert.equal(repositories.comments.patterns[0].name, 'comment.line.double-slash.mumei');
assert.equal(repositories.strings.patterns[0].name, 'string.quoted.double.mumei');
assert.equal(repositories.numbers.patterns[0].name, 'constant.numeric.mumei');
assert.equal(repositories.operators.patterns[0].name, 'keyword.operator.mumei');

console.log('VS Code extension assets validated');
