import { Graph } from "./components/Graph";
import { Chat } from "./components/Chat";

function App() {
  return (
    <div className="flex h-screen bg-zinc-950 text-zinc-100">
      {/* Sidebar graph */}
      <div className="w-72 shrink-0">
        <Graph />
      </div>
      {/* Main chat panel */}
      <div className="flex-1 min-w-0">
        <Chat />
      </div>
    </div>
  );
}

export default App;
