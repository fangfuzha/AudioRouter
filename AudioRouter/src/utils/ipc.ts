import type { Result } from "../generated/bindings";

/**
 * Unwraps a Result<T, string> from IPC calls.
 *
 * @param resultPromise - Promise that resolves to a Result type
 * @returns The unwrapped data if successful
 * @throws Error with the error message if the result status is "error"
 *
 * @example
 * ```typescript
 * try {
 *   const config = await unwrap(commands.getConfig());
 *   // Use config.data directly
 * } catch (e) {
 *   console.error("Failed:", e);
 * }
 * ```
 */
export async function unwrap<T>(
  resultPromise: Promise<Result<T, string>>,
): Promise<T> {
  const result = await resultPromise;
  if (result.status === "ok") {
    return result.data;
  }
  throw new Error(result.error);
}
