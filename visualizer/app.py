import streamlit as st
import json
import os
from pathlib import Path

st.set_page_config(page_title="Mumei Visualizer", page_icon="🗡️", layout="wide")

st.title("🗡️ Mumei Visualizer")
st.subheader("Formal Verification Inspection Dashboard")

# --- サイドバー: 表示モード選択 ---
view_mode = st.sidebar.radio(
    "表示モード",
    ["最新レポート", "Self-Healing 履歴"],
    index=0,
)

# --- 最新レポート表示 ---
if view_mode == "最新レポート":
    report_path = Path(__file__).parent / "report.json"
    if not report_path.exists():
        # フォールバック: カレントディレクトリの report.json
        report_path = Path("report.json")

    if not report_path.exists():
        st.info(
            "No verification reports found. Run the Mumei compiler first.\n\n"
            "```bash\n"
            "mumei build your_file.mm -o katana\n"
            "# or with Visualizer sync:\n"
            "ENABLE_VISUALIZER_SYNC=true python mcp_server.py\n"
            "```"
        )
        st.stop()

    try:
        with open(report_path, "r") as f:
            data = json.load(f)
    except (json.JSONDecodeError, OSError) as e:
        st.error(f"Failed to read report.json: {e}")
        st.stop()

    # 状態の表示
    if data.get("status") == "failed":
        st.error(
            f"❌ Verification Failed: Atom '{data.get('atom', 'unknown')}' is flawed."
        )

        # --- counterexample フィールド対応 ---
        if "counterexample" in data and data["counterexample"]:
            st.subheader("🔬 Z3 Counter-example (詳細)")
            ce = data["counterexample"]
            cols = st.columns(min(len(ce), 4))
            for i, (var_name, var_value) in enumerate(ce.items()):
                with cols[i % len(cols)]:
                    st.metric(f"Counter-example: {var_name}", var_value)
        else:
            # 従来の input_a / input_b フォールバック
            col1, col2 = st.columns(2)
            with col1:
                st.metric("Counter-example: a", data.get("input_a", "N/A"))
            with col2:
                st.metric("Counter-example: b", data.get("input_b", "N/A"))

        st.warning(f"**Reason:** {data.get('reason', 'Unknown')}")

        # AIへの修正指示用プロンプトの自動生成
        ce_info = ""
        if "counterexample" in data and data["counterexample"]:
            ce_info = "\n".join(
                f"    {k} = {v}" for k, v in data["counterexample"].items()
            )
        else:
            ce_info = (
                f"    a = {data.get('input_a', 'N/A')},"
                f" b = {data.get('input_b', 'N/A')}"
            )

        st.code(
            f"# AI Fix Suggestion:\n"
            f"The atom '{data.get('atom', 'unknown')}' failed verification.\n"
            f"Counter-example values:\n{ce_info}\n"
            f"Please update the 'requires' clause to handle this case.",
            language="markdown",
        )

    elif data.get("status") == "success":
        st.success(
            f"✅ Atom '{data.get('atom', 'unknown')}' is mathematically pure."
        )
        st.json(data)
    else:
        st.warning(f"Unknown status: {data.get('status')}")
        st.json(data)


# --- Self-Healing 履歴表示 ---
elif view_mode == "Self-Healing 履歴":
    history_path = Path(__file__).parent / "report_history.json"

    if not history_path.exists():
        st.info(
            "No self-healing history found.\n\n"
            "履歴を記録するには `ENABLE_VISUALIZER_SYNC=true` を設定してください。\n\n"
            "```bash\n"
            "# .env に追加\n"
            "ENABLE_VISUALIZER_SYNC=true\n"
            "```"
        )
        st.stop()

    try:
        with open(history_path, "r") as f:
            history = json.load(f)
    except (json.JSONDecodeError, OSError) as e:
        st.error(f"Failed to read report_history.json: {e}")
        st.stop()

    if not history:
        st.info("履歴が空です。")
        st.stop()

    st.metric("Total Iterations", len(history))

    # 成功/失敗の集計
    success_count = sum(1 for h in history if h.get("status") == "success")
    fail_count = sum(1 for h in history if h.get("status") == "failed")

    col1, col2, col3 = st.columns(3)
    with col1:
        st.metric("Passed", success_count)
    with col2:
        st.metric("Failed", fail_count)
    with col3:
        st.metric("Other", len(history) - success_count - fail_count)

    # 時系列表示
    st.subheader("検証履歴 (時系列)")
    for i, entry in enumerate(reversed(history)):
        idx = len(history) - i
        status_icon = "OK" if entry.get("status") == "success" else "NG"
        timestamp = entry.get("timestamp", "N/A")
        atom_name = entry.get("atom", "unknown")

        with st.expander(
            f"{status_icon} #{idx} -- {atom_name} ({timestamp})",
            expanded=(i == 0),
        ):
            st.json(entry)

            # counterexample の詳細表示
            if "counterexample" in entry and entry["counterexample"]:
                st.subheader("Counter-example")
                for var_name, var_value in entry["counterexample"].items():
                    st.code(f"{var_name} = {var_value}")

    # 履歴クリアボタン
    st.divider()
    if st.button("履歴をクリア", type="secondary"):
        history_path.write_text("[]", encoding="utf-8")
        st.rerun()
