// truss-code lazily imports Monaco (and its language workers) for syntax colouring. ruckcode renders
// diffs with `highlight={false}` and never loads it, but the bundler would still emit ~14 MB of
// chunks for it. vite.config.ts aliases monaco-editor to this stub so the desktop bundle stays small.
// TODO(truss-ui#142): make Monaco an optional peer of @ruckc/truss-code so this alias is not needed.
export default class MonacoStub {}
export const editor = {};
export const languages = {};
