import { createClient } from "@recordem/sdk";
import { accessToken } from "./auth";

export const api = createClient({ baseUrl: __API_BASE__ || location.origin, token: accessToken });
