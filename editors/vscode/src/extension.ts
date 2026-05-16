import * as vscode from 'vscode';
import { LanguageClient, LanguageClientOptions, ServerOptions } from 'vscode-languageclient/node';

let client: LanguageClient | undefined;
let ceDecorationType: vscode.TextEditorDecorationType | undefined;

export function activate(context: vscode.ExtensionContext) {
    const config = vscode.workspace.getConfiguration('mumei');
    const serverPath = config.get<string>('serverPath', 'mumei');
    const serverOptions: ServerOptions = {
        command: serverPath,
        args: ['lsp'],
    };
    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'mumei' }],
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.mm'),
        },
    };
    client = new LanguageClient(
        'mumei-lsp',
        'Mumei Language Server',
        serverOptions,
        clientOptions
    );
    client.start();

    context.subscriptions.push(
        vscode.commands.registerCommand(
            'mumei.showIntentDrift',
            (uriArg: unknown, atomArg: unknown, scoreArg: unknown) => {
                showIntentDrift(uriArg, atomArg, scoreArg);
            }
        ),
        vscode.commands.registerCommand(
            'mumei.showSpecCodeMapping',
            (uriArg: unknown, atomArg: unknown, clauseArg: unknown) => {
                showSpecCodeMapping(uriArg, atomArg, clauseArg);
            }
        )
    );

    // Plan 10 (Task 5D): Counter-example inline ghost-text decoration.
    //   The Mumei LSP attaches Z3 counter-examples to verification diagnostics.
    //   We render them as italic, after-line ghost text right next to the
    //   error so users can see the violating concrete inputs without opening
    //   a side panel.
    ceDecorationType = vscode.window.createTextEditorDecorationType({
        after: {
            color: '#e94560',
            fontStyle: 'italic',
            margin: '0 0 0 1em',
        },
        isWholeLine: false,
    });
    context.subscriptions.push(ceDecorationType);

    const refreshAllVisible = () => {
        for (const editor of vscode.window.visibleTextEditors) {
            if (editor.document.languageId === 'mumei') {
                updateCounterexampleDecorations(editor);
            }
        }
    };

    context.subscriptions.push(
        vscode.languages.onDidChangeDiagnostics((e) => {
            for (const uri of e.uris) {
                const editor = vscode.window.visibleTextEditors.find(
                    (ed) => ed.document.uri.toString() === uri.toString()
                );
                if (editor) {
                    updateCounterexampleDecorations(editor);
                }
            }
        })
    );

    context.subscriptions.push(
        vscode.window.onDidChangeActiveTextEditor((editor) => {
            if (editor && editor.document.languageId === 'mumei') {
                updateCounterexampleDecorations(editor);
            }
        })
    );

    context.subscriptions.push(
        vscode.window.onDidChangeVisibleTextEditors(() => {
            refreshAllVisible();
        })
    );

    refreshAllVisible();
}

function showIntentDrift(uriArg: unknown, atomArg: unknown, scoreArg: unknown) {
    const atom = textOrFallback(atomArg, 'selected atom');
    const score = numericScore(scoreArg);
    const scoreText = score === undefined ? 'unknown' : score.toFixed(2);
    const risk = score === undefined ? 'Unknown' : intentRisk(score);

    showDetailsPanel('mumeiIntentDrift', `Intent Drift: ${atom}`, [
        ['Atom', atom],
        ['Score', scoreText],
        ['Risk', risk],
        ['Document', uriText(uriArg)],
        ['Next step', 'Review requires/ensures clauses against implementation body.'],
    ]);
}

function showSpecCodeMapping(uriArg: unknown, atomArg: unknown, clauseArg: unknown) {
    const atom = textOrFallback(atomArg, 'selected atom');
    const clause = textOrFallback(clauseArg, 'contract');

    showDetailsPanel('mumeiSpecCodeMapping', `Spec-Code Mapping: ${clause}`, [
        ['Atom', atom],
        ['Clause', clause],
        ['Document', uriText(uriArg)],
        ['Mapping', `${clause} clause ↔ ${atom} implementation`],
        ['Next step', 'Inspect diagnostics, counter-examples, and proof artifacts for this atom.'],
    ]);
}

function showDetailsPanel(panelType: string, title: string, rows: Array<[string, string]>) {
    const panel = vscode.window.createWebviewPanel(
        panelType,
        title,
        vscode.ViewColumn.Beside,
        { enableScripts: false }
    );
    const body = rows
        .map(([label, value]) => `<tr><th>${escapeHtml(label)}</th><td>${escapeHtml(value)}</td></tr>`)
        .join('');
    panel.webview.html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<style>
body { font-family: var(--vscode-font-family); padding: 1rem; }
table { border-collapse: collapse; width: 100%; }
th, td { border-bottom: 1px solid var(--vscode-panel-border); padding: 0.5rem; text-align: left; vertical-align: top; }
th { width: 9rem; color: var(--vscode-descriptionForeground); }
</style>
<title>${escapeHtml(title)}</title>
</head>
<body>
<h1>${escapeHtml(title)}</h1>
<table>${body}</table>
</body>
</html>`;
}

function textOrFallback(value: unknown, fallback: string): string {
    return typeof value === 'string' && value.length > 0 ? value : fallback;
}

function numericScore(value: unknown): number | undefined {
    if (typeof value === 'number' && Number.isFinite(value)) {
        return value;
    }
    if (typeof value === 'string') {
        const parsed = Number(value);
        if (Number.isFinite(parsed)) {
            return parsed;
        }
    }
    return undefined;
}

function intentRisk(score: number): string {
    if (score <= 0.30) {
        return 'Low';
    }
    if (score < 0.75) {
        return 'Medium';
    }
    return 'High';
}

function uriText(value: unknown): string {
    if (value instanceof vscode.Uri) {
        return value.toString();
    }
    if (typeof value === 'string' && value.length > 0) {
        return value;
    }
    const editor = vscode.window.activeTextEditor;
    return editor ? editor.document.uri.toString() : 'unknown';
}

function escapeHtml(value: string): string {
    return value
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

function updateCounterexampleDecorations(editor: vscode.TextEditor) {
    if (!ceDecorationType) {
        return;
    }
    const diagnostics = vscode.languages.getDiagnostics(editor.document.uri);
    const decorations: vscode.DecorationOptions[] = [];

    for (const diag of diagnostics) {
        const ceText = extractCounterexampleText(diag);
        if (ceText) {
            decorations.push({
                range: diag.range,
                renderOptions: {
                    after: {
                        contentText: `  // ${ceText}`,
                    },
                },
            });
        }
    }

    editor.setDecorations(ceDecorationType, decorations);
}

function extractCounterexampleText(diag: vscode.Diagnostic): string | undefined {
    // Prefer structured `data.counterexample` if the LSP attached one.
    const data: unknown = (diag as unknown as { data?: unknown }).data;
    if (data && typeof data === 'object') {
        const ce = (data as { counterexample?: unknown }).counterexample;
        if (ce && typeof ce === 'object') {
            const parts: string[] = [];
            for (const [k, v] of Object.entries(ce as Record<string, unknown>)) {
                const value = typeof v === 'string' ? v : JSON.stringify(v);
                parts.push(`${k} = ${value}`);
            }
            if (parts.length > 0) {
                return parts.join(', ');
            }
        }
    }
    // Fallback: scrape "Counter-example: …" out of the diagnostic message.
    const match = diag.message.match(/Counter-example:\s*(.+)$/m);
    if (match) {
        return match[1].trim();
    }
    return undefined;
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) {
        return undefined;
    }
    return client.stop();
}
