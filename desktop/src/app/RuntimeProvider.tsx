import { createContext, useCallback, useContext, useEffect, useMemo, useRef, type ReactNode } from "react";
import type { StoreApi } from "zustand/vanilla";
import { CommandKind, type Envelope } from "../shared/protocol";
import { createPlatform, type PlatformApi, type SidecarStatus, type StorageHealth } from "../platform";
import { createSessionStore, type SessionStoreState } from "../stores/sessionStore";

type RuntimeContextValue = {
  platform: PlatformApi;
  store: StoreApi<SessionStoreState>;
  send(command: Envelope, questionTurnId?: string): Promise<void>;
  restart(): Promise<SidecarStatus | null>;
};

type SubscriptionLifecycle = {
  users: number;
  disposed: boolean;
  releaseTimer: ReturnType<typeof setTimeout> | null;
  unlisten: (() => void) | null;
  ready: Promise<void>;
};

type RestoreLifecycle = {
  users: number;
  releaseTimer: ReturnType<typeof setTimeout> | null;
  version: number;
};

type StorageHealthLifecycle = {
  users: number;
  disposed: boolean;
  releaseTimer: ReturnType<typeof setTimeout> | null;
  unlisten: (() => void) | null;
  eventRevision: number;
  ready: Promise<void>;
};

const RuntimeContext = createContext<RuntimeContextValue | null>(null);

export type RuntimeProviderProps = {
  children: ReactNode;
  platform?: PlatformApi;
};

export function RuntimeProvider({ children, platform: suppliedPlatform }: RuntimeProviderProps) {
  const platformRef = useRef<PlatformApi | null>(null);
  const storeRef = useRef<StoreApi<SessionStoreState> | null>(null);
  const lifecycleRef = useRef<SubscriptionLifecycle | null>(null);
  const storageHealthLifecycleRef = useRef<StorageHealthLifecycle | null>(null);
  const restoreLifecycleRef = useRef<RestoreLifecycle | null>(null);
  const startedRef = useRef(false);

  if (!platformRef.current) {
    platformRef.current = suppliedPlatform ?? createPlatform();
  }
  if (!storeRef.current) {
    storeRef.current = createSessionStore();
  }

  const platform = platformRef.current;
  const store = storeRef.current;

  useEffect(() => {
    let lifecycle = restoreLifecycleRef.current;
    if (!lifecycle) {
      lifecycle = { users: 0, releaseTimer: null, version: 0 };
      restoreLifecycleRef.current = lifecycle;
    }
    lifecycle.users += 1;
    if (lifecycle.releaseTimer !== null) {
      clearTimeout(lifecycle.releaseTimer);
      lifecycle.releaseTimer = null;
    }
    if (startedRef.current) {
      return () => releaseRestoreLifecycle(lifecycle!, restoreLifecycleRef);
    }
    startedRef.current = true;
    const restoreVersion = ++lifecycle.version;
    const restoreRevision = store.getState().sessionRevision;
    const isCurrent = () => lifecycle!.users > 0 && lifecycle!.version === restoreVersion;

    void platform.sidecarStatus()
      .then((status) => {
        if (isCurrent()) store.getState().setSidecarStatus(status);
      })
      .catch((error) => {
        if (isCurrent()) store.getState().recordError(error);
      });

    void platform.restoreActiveSession()
      .then(async (session) => {
        if (!session || !isCurrent() || store.getState().sessionRevision !== restoreRevision || store.getState().session) {
          return;
        }
        store.getState().restoreSession(session);
        const restoredRevision = store.getState().sessionRevision;
        const associations = await platform.getRequestTurnAssociations(session.id);
        if (!isCurrent() || store.getState().session?.id !== session.id || store.getState().sessionRevision !== restoredRevision) {
          return;
        }
        for (const association of associations) {
          store.getState().associateRequestWithTurn(association.requestId, association.turnId);
        }
        const timeline = await platform.getTimeline(session.id);
        if (!isCurrent() || store.getState().session?.id !== session.id || store.getState().sessionRevision !== restoredRevision) {
          return;
        }
        store.getState().restoreReplay(timeline);
      })
      .catch((error) => {
        if (isCurrent()) store.getState().recordError(error);
      });

    return () => releaseRestoreLifecycle(lifecycle!, restoreLifecycleRef);
  }, [platform, store]);

  useEffect(() => {
    let lifecycle = storageHealthLifecycleRef.current;
    if (!lifecycle) {
      lifecycle = {
        users: 0,
        disposed: false,
        releaseTimer: null,
        unlisten: null,
        eventRevision: 0,
        ready: Promise.resolve(),
      };
      lifecycle.ready = platform.subscribeStorageHealth((health: StorageHealth) => {
        lifecycle!.eventRevision += 1;
        if (isSubscriptionActive(lifecycle!)) {
          store.getState().setStorageHealth(health);
        }
      })
        .then(async (unlisten) => {
          lifecycle!.unlisten = unlisten;
          if (!isSubscriptionActive(lifecycle!)) {
            unlisten();
            lifecycle!.unlisten = null;
            return;
          }
          const eventRevision = lifecycle!.eventRevision;
          const health = await platform.storageHealth();
          if (isSubscriptionActive(lifecycle!) && lifecycle!.eventRevision === eventRevision) {
            store.getState().setStorageHealth(health);
          }
        })
        .catch((error) => {
          if (isSubscriptionActive(lifecycle!)) store.getState().recordError(error);
        });
      storageHealthLifecycleRef.current = lifecycle;
    }
    lifecycle.users += 1;
    if (lifecycle.releaseTimer !== null) {
      clearTimeout(lifecycle.releaseTimer);
      lifecycle.releaseTimer = null;
    }

    return () => releaseSubscriptionLifecycle(lifecycle!, storageHealthLifecycleRef);
  }, [platform, store]);

  useEffect(() => {
    let lifecycle = lifecycleRef.current;
    if (!lifecycle) {
      lifecycle = {
        users: 0,
        disposed: false,
        releaseTimer: null,
        unlisten: null,
        ready: Promise.resolve(),
      };
      lifecycle.ready = platform.subscribe((event) => {
        if (isSubscriptionActive(lifecycle!)) store.getState().applyEnvelope(event);
      })
        .then((unlisten) => {
          lifecycle!.unlisten = unlisten;
          if (!isSubscriptionActive(lifecycle!)) {
            unlisten();
            lifecycle!.unlisten = null;
          }
        })
        .catch((error) => {
          if (isSubscriptionActive(lifecycle!)) store.getState().recordError(error);
        });
      lifecycleRef.current = lifecycle;
    }
    lifecycle.users += 1;
    if (lifecycle.releaseTimer !== null) {
      clearTimeout(lifecycle.releaseTimer);
      lifecycle.releaseTimer = null;
    }

    return () => {
      lifecycle!.users -= 1;
      if (lifecycle!.users !== 0) {
        return;
      }
      lifecycle!.releaseTimer = setTimeout(() => {
        if (lifecycle!.users !== 0) {
          return;
        }
        lifecycle!.disposed = true;
        lifecycle!.unlisten?.();
        lifecycle!.unlisten = null;
        lifecycleRef.current = null;
      }, 0);
    };
  }, [platform, store]);

  const send = useCallback(async (command: Envelope, questionTurnId?: string) => {
    if (command.kind === CommandKind.QUERY_TRIGGER) {
      if (!questionTurnId) {
        const error = new Error("query request requires a question turn association");
        store.getState().recordError(error);
        throw error;
      }
      const activeSession = store.getState().session;
      if (!activeSession || activeSession.status !== "active" || activeSession.id !== command.session_id) {
        const error = new Error("query request must target the active session before associating a question turn");
        store.getState().recordError(error);
        throw error;
      }
      try {
        await platform.associateRequestWithTurn({
          sessionId: activeSession.id,
          requestId: command.id,
          turnId: questionTurnId,
        });
        store.getState().associateRequestWithTurn(command.id, questionTurnId);
      } catch (error) {
        store.getState().recordError(error);
        throw error;
      }
    }
    try {
      await platform.send(command);
    } catch (error) {
      store.getState().recordError(error);
      throw error;
    }
  }, [platform, store]);

  const restart = useCallback(async () => {
    try {
      const status = await platform.restartSidecar();
      store.getState().clearTransientState();
      store.getState().setSidecarStatus(status);
      return status;
    } catch (error) {
      store.getState().recordError(error);
      return null;
    }
  }, [platform, store]);

  const value = useMemo<RuntimeContextValue>(() => ({ platform, store, send, restart }), [platform, restart, send, store]);

  return <RuntimeContext.Provider value={value}>{children}</RuntimeContext.Provider>;
}

function releaseRestoreLifecycle(
  lifecycle: RestoreLifecycle,
  reference: { current: RestoreLifecycle | null },
) {
  lifecycle.users -= 1;
  if (lifecycle.users !== 0) return;
  lifecycle.releaseTimer = setTimeout(() => {
    if (lifecycle.users !== 0) return;
    lifecycle.version += 1;
    reference.current = null;
  }, 0);
}

function isSubscriptionActive(lifecycle: SubscriptionLifecycle | StorageHealthLifecycle) {
  return lifecycle.users > 0 && !lifecycle.disposed;
}

function releaseSubscriptionLifecycle(
  lifecycle: SubscriptionLifecycle | StorageHealthLifecycle,
  reference: { current: SubscriptionLifecycle | StorageHealthLifecycle | null },
) {
  lifecycle.users -= 1;
  if (lifecycle.users !== 0) return;
  lifecycle.releaseTimer = setTimeout(() => {
    if (lifecycle.users !== 0) return;
    lifecycle.disposed = true;
    lifecycle.unlisten?.();
    lifecycle.unlisten = null;
    reference.current = null;
  }, 0);
}

export function useRuntime(): RuntimeContextValue {
  const runtime = useContext(RuntimeContext);
  if (!runtime) {
    throw new Error("useRuntime must be used inside RuntimeProvider");
  }
  return runtime;
}
