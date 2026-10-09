---
name: corex-fim
description: >-
  Use this skill when the user asks about FIM (Fill-in-the-Middle) code
  autocompletion in Corex, how to use /fim, how to complete code at the
  cursor position, or how to use Corex for code autocomplete. Trigger
  phrases: 'fim', 'fill in the middle', 'autocomplete', '/fim', 'complete
  this code', 'code completion', 'inline completion'.
---

# Corex FIM — Fill-in-the-Middle Code Completion

FIM (Fill-in-the-Middle) is a specialized inference mode where the model completes code between a **prefix** (code before cursor) and a **suffix** (code after cursor). It produces precise inline completions rather than conversational responses.

---

## 🚀 Usage

### From the TUI
```
/fim <file-path>
```

Example:
```
/fim src/main.rs
/fim crates/corex-core/src/config.rs
/fim ./my_script.py
```

Corex will:
1. Read the file content
2. Ask you where to insert the completion (cursor position / line)
3. Send a FIM request to the DeepSeek API
4. Return the generated code to insert at that position

---

## 🧠 How FIM Works

Unlike a normal chat request, FIM uses a special prompt format:

```
<|fim_prefix|>  ← everything before the cursor
<|fim_suffix|>  ← everything after the cursor  
<|fim_middle|>  ← model fills this in
```

This means the model sees **both sides** of where you want the completion — giving it full context to generate accurate code that fits your existing structure.

---

## 🔧 Models for FIM

- **`deepseek-flash`** (DeepSeek-V4.1-Flash) — supports FIM natively, fast responses
- **`deepseek-v4-pro`** — use for complex multi-line completions requiring deep reasoning

Switch model before running FIM:
```
/model
```

---

## 💡 Best Practices

1. **Point to a specific file** — `/fim src/handler.rs` gives better context than pasting code
2. **Be specific about the cursor position** — tell Corex which line or function to complete
3. **Use with small focused functions** — FIM excels at completing single functions, docstrings, or test bodies
4. **Combine with `/plan` first** for large refactors — plan the structure, then use FIM to fill in each function

---

## 📝 Examples

```
/fim src/auth.rs
→ Complete the validate_token() function at line 45

/fim tests/integration_test.rs  
→ Fill in the test body for test_api_endpoint()

/fim config/schema.json
→ Complete the missing fields in the user object
```

---

## ⚠️ Notes

- FIM is different from asking the model to "write" code — it is designed for **insertion** at a specific point
- For wholesale rewrites or new files, use a normal chat prompt instead
- FIM requests bypass the tool loop — the model returns the completion directly without executing tools
