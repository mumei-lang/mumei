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
