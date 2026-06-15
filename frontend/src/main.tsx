import {createRoot} from "react-dom/client";import {QueryClient,QueryClientProvider} from "@tanstack/react-query";import {App} from "./App";import "./index.css";
createRoot(document.getElementById("root")!).render(<QueryClientProvider client={new QueryClient()}><App/></QueryClientProvider>);
