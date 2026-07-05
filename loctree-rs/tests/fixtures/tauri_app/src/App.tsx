// Frontend Tauri app
import { invoke } from '@tauri-apps/api/core';

export function App() {
    const handleGreet = async () => {
        const result = await invoke('greet', { name: 'World' });
        console.log(result);
    };

    const handleSave = async () => {
        await invoke('save_data', { data: 'test' });
    };

    // This handler doesn't exist in backend!
    const handleMissing = async () => {
        await invoke('missing_handler', { foo: 'bar' });
    };

    return { handleGreet, handleSave, handleMissing };
}
