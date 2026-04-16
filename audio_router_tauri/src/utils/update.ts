import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-shell";

/**
 * 查询 GitHub Release 最新版本并在发现新版本时打开对应页面。
 * UI 层负责调用此方法并处理弹窗提示等。
 */
export async function checkForUpdates(): Promise<void> {
  try {
    const resp = await fetch(
      "https://api.github.com/repos/fangfuzha/AudioRouter/releases/latest",
    );
    if (!resp.ok) return;
    const data = await resp.json();
    const latestRaw: string = data.tag_name;
    const currentRaw = await getVersion();
    // strip leading 'v' or 'V' for comparison
    const normalize = (v: string) => v.replace(/^[vV]/, "");
    const latest = normalize(latestRaw);
    const current = normalize(currentRaw);
    if (latest && current && latest !== current) {
      if (confirm("检测到新版本，前往 GitHub Releases 下载？")) {
        open(data.html_url);
      }
    }
  } catch (e) {
    console.error("update check failed", e);
  }
}
