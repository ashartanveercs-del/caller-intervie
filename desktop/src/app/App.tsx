import { RouterProvider } from "react-router-dom";
import { router } from "./router";
import { RuntimeProvider } from "./RuntimeProvider";

export function App() {
  return (
    <RuntimeProvider>
      <RouterProvider router={router} />
    </RuntimeProvider>
  );
}
