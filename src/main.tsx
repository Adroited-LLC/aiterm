import ReactDOM from "react-dom/client";
import App from "./App";

// No StrictMode: its double-mounted effects would spawn/kill a PTY per terminal.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
