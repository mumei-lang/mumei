# Editor Configuration for Mumei LSP

The Mumei language server (`mumei lsp`) communicates via JSON-RPC over stdio and provides:

- **Diagnostics** — parse errors and Z3 verification errors
- **Hover** — atom requires/ensures/effects display
- **Completion** — keywords, atom names, effect names, type/struct/enum names
- **Go to Definition** — jump to atom, type, struct, enum, and effect definitions

## Neovim

### Using `nvim-lspconfig`

Add to your Neovim configuration (e.g., `~/.config/nvim/init.lua`):

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")

if not configs.mumei then
  configs.mumei = {
    default_config = {
      cmd = { "mumei", "lsp" },
      filetypes = { "mumei" },
      root_dir = lspconfig.util.root_pattern("mumei.toml", ".git"),
      settings = {},
    },
  }
end

lspconfig.mumei.setup({})
```

Also register the `.mm` file type:

```lua
vim.filetype.add({
  extension = {
    mm = "mumei",
  },
})
```

### Using native `vim.lsp.start`

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "mumei",
  callback = function()
    vim.lsp.start({
      name = "mumei-lsp",
      cmd = { "mumei", "lsp" },
      root_dir = vim.fs.dirname(vim.fs.find({ "mumei.toml", ".git" }, { upward = true })[1]),
    })
  end,
})
```

---

## Helix

Add to `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "mumei"
scope = "source.mumei"
file-types = ["mm"]
roots = ["mumei.toml"]
language-servers = ["mumei-lsp"]

[language-server.mumei-lsp]
command = "mumei"
args = ["lsp"]
```

---

## Emacs (eglot)

Add to your Emacs configuration (e.g., `~/.emacs.d/init.el`):

```elisp
;; Register .mm files as mumei-mode
(define-derived-mode mumei-mode prog-mode "Mumei"
  "Major mode for editing Mumei (.mm) files.")

(add-to-list 'auto-mode-alist '("\\.mm\\'" . mumei-mode))

;; Configure eglot for mumei
(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs
               '(mumei-mode . ("mumei" "lsp"))))

;; Auto-start eglot for mumei files
(add-hook 'mumei-mode-hook #'eglot-ensure)
```

---

## Sublime Text

Install the [LSP](https://packagecontrol.io/packages/LSP) package, then add a client configuration.

**Settings** → **Package Settings** → **LSP** → **Settings**:

```json
{
  "clients": {
    "mumei": {
      "enabled": true,
      "command": ["mumei", "lsp"],
      "selector": "source.mumei",
      "schemes": ["file"]
    }
  }
}
```

Also create a syntax definition for `.mm` files or associate them with the `mumei` scope via **View** → **Syntax** → **Open all with current extension as…**.

---

## Zed

Add to your Zed settings (`~/.config/zed/settings.json`):

```json
{
  "languages": {
    "Mumei": {
      "language_servers": ["mumei-lsp"]
    }
  },
  "lsp": {
    "mumei-lsp": {
      "binary": {
        "path": "mumei",
        "arguments": ["lsp"]
      }
    }
  }
}
```

Register the `.mm` extension by placing a language configuration in your Zed extensions directory or by adding:

```json
{
  "file_types": {
    "Mumei": ["mm"]
  }
}
```

---

## Verifying the Setup

After configuring your editor, open a `.mm` file and verify:

1. **Diagnostics**: Introduce a syntax error (e.g., remove a semicolon) and confirm that the editor shows a red squiggly or error marker.
2. **Completion**: Type `ato` and trigger completion — you should see `atom` and `atom_ref` in the completion list.
3. **Hover**: Hover over an `atom` definition to see its `requires` and `ensures` contracts.
4. **Go to Definition**: Place the cursor on an atom name and use your editor's "Go to Definition" command to jump to its definition.
