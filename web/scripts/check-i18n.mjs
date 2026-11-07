// Checks that the text of the web client is marked for translation (A11Y-04): every piece of
// template text, and every attribute that people read or hear, has an `i18n` marker, so that
// `ng extract-i18n` finds it. Text in TypeScript uses `$localize`, which review checks.
//
// It also checks that text takes the product name from `PRODUCT_NAME` instead of spelling it
// out, where translators could change it (UI-01).
//
// Templates must be .html files, so that this check sees all of them.
//
// Usage: node scripts/check-i18n.mjs (from web/). Exits with 1 and lists the problems if any.

import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { parseTemplate, tmplAstVisitAll, TmplAstRecursiveVisitor } from '@angular/compiler';

const SOURCE = 'src';
/** Attributes whose text people read or hear. */
const TEXT_ATTRIBUTES = new Set([
  'alt',
  'aria-description',
  'aria-label',
  'aria-placeholder',
  'aria-roledescription',
  'aria-valuetext',
  'label',
  'placeholder',
  'title',
]);
const LETTER = /\p{L}/u;
const INTERPOLATION = /\{\{[\s\S]*?\}\}/g;
const PRODUCT_FILE = join(SOURCE, 'app', 'product.ts');
const PRODUCT_NAME = readFileSync(PRODUCT_FILE, 'utf8').match(/PRODUCT_NAME = '([^']+)'/)?.[1];
if (!PRODUCT_NAME) {
  throw new Error(`PRODUCT_NAME not found in ${PRODUCT_FILE}`);
}
const NAME_PROBLEM = `the product name in text; use PRODUCT_NAME from ${PRODUCT_FILE}`;
/** A `$localize` text, up to its closing backtick. */
const LOCALIZE = /\$localize`(?:[^`\\]|\\.)*`/g;

class Checker extends TmplAstRecursiveVisitor {
  /** How many of the enclosing elements are marked, so that their content is too. */
  marked = 0;
  problems = [];

  constructor(file) {
    super();
    this.file = file;
  }

  visitElement(element) {
    this.checkAttributes(element.attributes);
    this.within(element, () => super.visitElement(element));
  }

  visitTemplate(template) {
    this.checkAttributes(template.attributes);
    this.within(template, () => super.visitTemplate(template));
  }

  visitText(text) {
    if (!this.marked && LETTER.test(text.value)) {
      this.report(text, `text without i18n: "${text.value.trim()}"`);
    }
    if (text.value.includes(PRODUCT_NAME)) {
      this.report(text, NAME_PROBLEM);
    }
  }

  visitBoundText(text) {
    const literal = text.value.source.replace(INTERPOLATION, '');
    if (!this.marked && LETTER.test(literal)) {
      this.report(text, `text without i18n: "${literal.trim()}"`);
    }
    if (literal.includes(PRODUCT_NAME)) {
      this.report(text, NAME_PROBLEM);
    }
  }

  visitIcu(icu) {
    if (!this.marked) {
      this.report(icu, 'ICU expression without i18n');
    }
  }

  checkAttributes(attributes) {
    for (const attribute of attributes) {
      if (TEXT_ATTRIBUTES.has(attribute.name) && LETTER.test(attribute.value) && !attribute.i18n) {
        this.report(attribute, `${attribute.name} without i18n-${attribute.name}`);
      }
      if (TEXT_ATTRIBUTES.has(attribute.name) && attribute.value.includes(PRODUCT_NAME)) {
        this.report(attribute, NAME_PROBLEM);
      }
    }
  }

  within(node, visit) {
    const marks = node.i18n ? 1 : 0;
    this.marked += marks;
    visit();
    this.marked -= marks;
  }

  report(node, message) {
    const { line, col } = node.sourceSpan.start;
    this.problems.push(`${this.file}:${line + 1}:${col + 1}: ${message}`);
  }
}

const problems = [];
for (const entry of readdirSync(SOURCE, { recursive: true })) {
  const file = join(SOURCE, entry);
  if (file.endsWith('.ts') && !file.endsWith('.spec.ts')) {
    const code = readFileSync(file, 'utf8');
    if (/\btemplate\s*:/.test(code)) {
      problems.push(`${file}: inline template; use templateUrl`);
    }
    for (const text of code.matchAll(LOCALIZE)) {
      if (text[0].includes(PRODUCT_NAME)) {
        const line = code.slice(0, text.index).split('\n').length;
        problems.push(`${file}:${line}: ${NAME_PROBLEM}`);
      }
    }
  } else if (file.endsWith('.html') && file !== join(SOURCE, 'index.html')) {
    const parsed = parseTemplate(readFileSync(file, 'utf8'), file, { preserveWhitespaces: false });
    for (const error of parsed.errors ?? []) {
      problems.push(`${file}: ${error.toString()}`);
    }
    const checker = new Checker(file);
    tmplAstVisitAll(checker, parsed.nodes);
    problems.push(...checker.problems);
  }
}

if (problems.length > 0) {
  console.error(problems.join('\n'));
  console.error(
    `\n${problems.length} problem(s). Mark user-facing text with i18n, and write the product name as PRODUCT_NAME (web/README.md).`,
  );
  process.exit(1);
}
