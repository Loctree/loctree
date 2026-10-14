import assert from 'node:assert/strict';
import test from 'node:test';

import {
    LoctreeGateway,
    formatHealthScore,
    LoctreeNotRunningError,
    isSnapshotNotLoaded,
    type LspRuntimeState,
} from '../src/gateway';

type Request = {
    method: string;
    params: unknown;
};

test('unknown health is distinct from a measured zero', () => {
    assert.equal(formatHealthScore(null), 'unknown');
    assert.equal(formatHealthScore(0), '0/100');
    assert.equal(formatHealthScore(71), '71/100');
});

function createGateway(running = true) {
    const requests: Request[] = [];
    const client = {
        isRunning: () => running,
        sendRequest: async (method: string, params: unknown) => {
            requests.push({ method, params });
            return { method, params };
        },
        sendNotification: (method: string) => {
            requests.push({ method, params: undefined });
        },
    };

    return {
        gateway: new LoctreeGateway(() => client as never),
        requests,
    };
}

function createGatewayWithState(state: LspRuntimeState, running = true) {
    const requests: Request[] = [];
    const client = {
        isRunning: () => running,
        sendRequest: async (method: string, params: unknown) => {
            requests.push({ method, params });
            return { method, params };
        },
        sendNotification: (method: string) => {
            requests.push({ method, params: undefined });
        },
    };

    return {
        gateway: new LoctreeGateway(() => client as never, () => state),
        requests,
    };
}

test('LoctreeGateway maps literal search options to loctree/find params', async () => {
    const { gateway, requests } = createGateway();

    await gateway.literal('SnapshotRootStrategy', {
        limit: 25,
        whole_token: true,
        group_by_file: true,
        offset: 50,
        count_only: true,
        symbol_id: 'loctree-lsp/src/snapshot.rs::SnapshotRootStrategy',
    });

    assert.deepEqual(requests[0], {
        method: 'loctree/find',
        params: {
            query: 'SnapshotRootStrategy',
            mode: 'literal',
            lang: undefined,
            dead_only: false,
            exported_only: false,
            limit: 25,
            cursor: undefined,
            chunk_size: undefined,
            whole_token: true,
            group_by_file: true,
            offset: 50,
            count_only: true,
            slim: undefined,
            symbol_id: 'loctree-lsp/src/snapshot.rs::SnapshotRootStrategy',
        },
    });
});

test('LoctreeGateway maps symbolContext options to LSP snake_case params', async () => {
    const { gateway, requests } = createGateway();

    await gateway.symbolContext(
        'loctree-lsp/src/symbol_context.rs',
        { line: 12, character: 8 },
        {
            symbol: 'symbol_context',
            bodyMaxLines: 40,
            occurrenceLimit: 10,
            sameFileOnly: true,
            offset: 20,
            wholeToken: true,
        },
    );

    assert.deepEqual(requests[0], {
        method: 'loctree/symbolContext',
        params: {
            file: 'loctree-lsp/src/symbol_context.rs',
            position: { line: 12, character: 8 },
            symbol: 'symbol_context',
            body_max_lines: 40,
            occurrence_limit: 10,
            same_file_only: true,
            offset: 20,
            whole_token: true,
        },
    });
});

test('LoctreeGateway maps contextPack options and refresh notification', async () => {
    const { gateway, requests } = createGateway();

    await gateway.contextPack({
        cursor: 'risk:2',
        cards: ['core', 'risk'],
        task: 'pre-release audit',
    });
    gateway.refresh();

    assert.deepEqual(requests, [
        {
            method: 'loctree/contextPack',
            params: {
                cursor: 'risk:2',
                cards: ['core', 'risk'],
                task: 'pre-release audit',
            },
        },
        {
            method: 'loctree/refresh',
            params: undefined,
        },
    ]);
});

test('LoctreeGateway reports readiness and rejects requests when client is not running', async () => {
    const { gateway } = createGateway(false);

    assert.equal(gateway.isReady(), false);
    assert.throws(
        () => {
            void gateway.health();
        },
        (error) => error instanceof LoctreeNotRunningError,
    );
});

test('LoctreeGateway gates requests until initialize handshake is complete', async () => {
    const starting = createGatewayWithState({
        phase: 'starting',
        message: 'Starting loctree-lsp and waiting for initialize handshake.',
        serverCommand: '/tmp/loctree-lsp',
    });

    assert.equal(starting.gateway.isReady(), false);
    assert.throws(
        () => {
            void starting.gateway.health();
        },
        (error) =>
            error instanceof LoctreeNotRunningError &&
            error.state.phase === 'starting' &&
            error.message.includes('initialize handshake'),
    );
    assert.deepEqual(starting.requests, []);

    const running = createGatewayWithState({
        phase: 'running',
        message: 'Initialize handshake completed.',
        serverCommand: '/tmp/loctree-lsp',
    });

    assert.equal(running.gateway.isReady(), true);
    await running.gateway.health();
    assert.equal(running.requests[0]?.method, 'loctree/health');
});

test('LoctreeGateway exposes startup errors as the not-ready reason', () => {
    const { gateway } = createGatewayWithState({
        phase: 'error',
        message: 'Failed to start loctree-lsp at /missing/loctree-lsp: spawn ENOENT',
        serverCommand: '/missing/loctree-lsp',
        detail: 'exit code 127',
    });

    assert.equal(gateway.isReady(), false);
    assert.throws(
        () => {
            void gateway.contextPack();
        },
        (error) =>
            error instanceof LoctreeNotRunningError &&
            error.state.phase === 'error' &&
            error.message.includes('/missing/loctree-lsp') &&
            error.message.includes('exit code 127'),
    );
});

test('isSnapshotNotLoaded recognizes transient not-loaded LSP errors', () => {
    assert.equal(isSnapshotNotLoaded({ code: -32001 }), true);
    assert.equal(isSnapshotNotLoaded({ message: 'loctree snapshot not loaded yet' }), true);
    assert.equal(isSnapshotNotLoaded({ code: -32603, message: 'hard failure' }), false);
});
