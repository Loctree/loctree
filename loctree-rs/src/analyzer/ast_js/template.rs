//! Template parsing for Svelte and Vue frameworks.
//!
//! This module handles detection of function calls, event handlers, bindings,
//! and component usage within Svelte and Vue template syntax.
//!
//! 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents ⓒ 2025-2026 Loctree Team

use regex::Regex;

/// Parse Svelte template for function calls and variable references.
///
/// Detects:
/// - Function calls: `{funcName()}`
/// - Event handlers: `on:click={handler}` and arrow functions `on:click={() => save()}`
/// - Bind directives: `bind:value={varName}`
/// - Use directives: `use:action`
/// - Transitions: `transition:fade`, `in:fly`, `out:slide`
/// - Components: `<ComponentName />`
/// - Prop values: `propName={value}`
/// - Method calls with optional chaining: `obj?.method()`
pub(super) fn parse_svelte_template_usages(template: &str) -> Vec<String> {
    let mut usages = Vec::new();

    // Pattern 1: Function calls {funcName()} or {funcName(args)}
    if let Ok(re) = Regex::new(r#"\{[^}]*?\b([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\("#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 2: Event handlers on:click={handler}
    // Also captures arrow functions: on:click={() => save(item)}
    if let Ok(re) = Regex::new(r#"on:\w+\s*=\s*\{(?:\([^)]*\)\s*=>)?\s*([a-zA-Z_$][a-zA-Z0-9_$]*)"#)
    {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 2b: Extract function names from arrow function bodies
    // Matches: on:click={() => functionName(...)} or on:click={(e) => handler(e)}
    if let Ok(re) =
        Regex::new(r#"on:\w+\s*=\s*\{(?:\([^)]*\))?\s*=>\s*([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\("#)
    {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 3: Bind directives bind:value={varName}
    if let Ok(re) = Regex::new(r#"bind:\w+\s*=\s*\{([a-zA-Z_$][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 4: Use directives use:action
    if let Ok(re) = Regex::new(r#"use:([a-zA-Z_$][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 5: Transition directives transition:fade, in:fly, out:slide
    if let Ok(re) = Regex::new(r#"(?:transition|in|out|animate):([a-zA-Z_$][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 6: Component usage <ComponentName />
    if let Ok(re) = Regex::new(r#"<([A-Z][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 7: Prop passing propName={value}
    if let Ok(re) =
        Regex::new(r#"\s[a-zA-Z_$][a-zA-Z0-9_$]*\s*=\s*\{([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\}"#)
    {
        for caps in re.captures_iter(template) {
            if let Some(value) = caps.get(1) {
                let ident = value.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 8: Method calls with optional chaining - obj?.method() or obj.method()
    // This catches component method references like chatInput?.focusInput()
    if let Ok(re) =
        Regex::new(r#"([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\??\.\s*([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\("#)
    {
        for caps in re.captures_iter(template) {
            // Capture the object (e.g., chatInput)
            if let Some(obj) = caps.get(1) {
                let ident = obj.as_str().to_string();
                if !is_svelte_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
            // Capture the method name (e.g., focusInput)
            // This is critical for detecting Svelte component API methods called via bind:this
            if let Some(method) = caps.get(2) {
                let method_name = method.as_str().to_string();
                if !is_svelte_builtin(&method_name) && !usages.contains(&method_name) {
                    usages.push(method_name);
                }
            }
        }
    }

    usages
}

/// Parse Vue template for function calls and variable references.
///
/// Detects:
/// - Mustache interpolations: `{{ functionName(...) }}`
/// - Variable references: `{{ variable.property }}`
/// - Event handlers: `@click="handler"` or `v-on:click="handler"`
/// - Prop bindings: `:prop="value"` or `v-bind:prop="value"`
/// - v-model bindings
/// - Components: `<ComponentName />`
pub(super) fn parse_vue_template_usages(template: &str) -> Vec<String> {
    let mut usages = Vec::new();

    // Pattern 1: Mustache interpolations {{ functionName(...) }} - function calls
    if let Ok(re) = Regex::new(r#"\{\{[^}]*?\b([a-zA-Z_$][a-zA-Z0-9_$]*)\s*\("#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_vue_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 1b: Mustache interpolations {{ variable.property }} - variable references
    // Captures the root variable name (before the dot)
    if let Ok(re) = Regex::new(r#"\{\{\s*([a-zA-Z_$][a-zA-Z0-9_$]*)\.?"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_vue_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 2: Event handlers @click="handler" or v-on:click="handler"
    if let Ok(re) = Regex::new(r#"(?:@|v-on:)\w+\s*=\s*"([a-zA-Z_$][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_vue_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 3: Prop bindings :prop="computedValue" or v-bind:prop="value"
    if let Ok(re) = Regex::new(r#"(?::|v-bind:)\w+\s*=\s*"([a-zA-Z_$][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_vue_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 4: v-model bindings
    if let Ok(re) = Regex::new(r#"v-model\s*=\s*"([a-zA-Z_$][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !is_vue_builtin(&ident) && !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    // Pattern 5: Component usage <ComponentName />
    if let Ok(re) = Regex::new(r#"<([A-Z][a-zA-Z0-9_$]*)"#) {
        for caps in re.captures_iter(template) {
            if let Some(name) = caps.get(1) {
                let ident = name.as_str().to_string();
                if !usages.contains(&ident) {
                    usages.push(ident);
                }
            }
        }
    }

    usages
}

/// Check if an identifier is a Vue built-in or control flow keyword.
fn is_vue_builtin(name: &str) -> bool {
    matches!(
        name,
        "if" | "else"
            | "for"
            | "slot"
            | "component"
            | "transition"
            | "keep-alive"
            | "teleport"
            | "suspense"
            | "console"
            | "window"
            | "document"
            | "Array"
            | "Object"
            | "String"
            | "Number"
            | "Boolean"
            | "Date"
            | "Math"
            | "JSON"
            | "Promise"
            | "Error"
            | "undefined"
            | "null"
            | "true"
            | "false"
            | "this"
    )
}

/// Check if an identifier is a Svelte built-in or control flow keyword.
fn is_svelte_builtin(name: &str) -> bool {
    matches!(
        name,
        "if" | "else"
            | "each"
            | "await"
            | "then"
            | "catch"
            | "key"
            | "html"
            | "debug"
            | "const"
            | "let"
            | "var"
            | "console"
            | "window"
            | "document"
            | "Array"
            | "Object"
            | "String"
            | "Number"
            | "Boolean"
            | "Date"
            | "Math"
            | "JSON"
            | "Promise"
            | "Error"
            | "undefined"
            | "null"
            | "true"
            | "false"
            | "this"
            | "slot"
            | "svelte"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== SVELTE TEMPLATE TESTS ==========

    #[test]
    fn test_svelte_template_function_calls() {
        let template = r#"
            <div>
                <span>{badgeText(account)}</span>
                <p>{formatDate(date, 'short')}</p>
            </div>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"badgeText".to_string()));
        assert!(usages.contains(&"formatDate".to_string()));
    }

    #[test]
    fn test_svelte_template_event_handlers() {
        let template = r#"
            <button on:click={handleClick}>Click me</button>
            <input on:input={onInputChange} />
            <form on:submit={submitForm}>...</form>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"handleClick".to_string()));
        assert!(usages.contains(&"onInputChange".to_string()));
        assert!(usages.contains(&"submitForm".to_string()));
    }

    #[test]
    fn test_svelte_template_bind_directives() {
        let template = r#"
            <input bind:value={inputValue} />
            <select bind:value={selectedOption}>...</select>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"inputValue".to_string()));
        assert!(usages.contains(&"selectedOption".to_string()));
    }

    #[test]
    fn test_svelte_template_use_directives() {
        let template = r#"
            <div use:clickOutside use:tooltip={tooltipParams}>
                Content
            </div>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"clickOutside".to_string()));
        assert!(usages.contains(&"tooltip".to_string()));
    }

    #[test]
    fn test_svelte_template_transitions() {
        let template = r#"
            <div transition:fade in:fly out:slide animate:flip>
                Animated content
            </div>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"fade".to_string()));
        assert!(usages.contains(&"fly".to_string()));
        assert!(usages.contains(&"slide".to_string()));
        assert!(usages.contains(&"flip".to_string()));
    }

    #[test]
    fn test_svelte_template_components() {
        let template = r#"
            <MyComponent prop={value} />
            <AnotherWidget />
            <div><NestedComponent /></div>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"MyComponent".to_string()));
        assert!(usages.contains(&"AnotherWidget".to_string()));
        assert!(usages.contains(&"NestedComponent".to_string()));
    }

    #[test]
    fn test_svelte_template_control_flow_with_functions() {
        let template = r#"
            {#if hasConflicts()}
                <Warning />
            {/if}
            {#each getItems() as item}
                <Item {item} />
            {/each}
            {:else if checkCondition()}
                <Fallback />
            {/if}
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"hasConflicts".to_string()));
        assert!(usages.contains(&"getItems".to_string()));
        assert!(usages.contains(&"checkCondition".to_string()));
    }

    #[test]
    fn test_svelte_template_prop_values() {
        let template = r#"
            <Component value={myValue} handler={myHandler} />
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"myValue".to_string()));
        assert!(usages.contains(&"myHandler".to_string()));
    }

    #[test]
    fn test_svelte_builtins_not_detected() {
        let template = r#"
            {#if condition}
                {#each items as item}
                    {#await promise then value}
                        {console.log(value)}
                    {:catch error}
                        {error}
                    {/await}
                {/each}
            {/if}
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(!usages.contains(&"if".to_string()));
        assert!(!usages.contains(&"each".to_string()));
        assert!(!usages.contains(&"await".to_string()));
        assert!(!usages.contains(&"then".to_string()));
        assert!(!usages.contains(&"catch".to_string()));
        assert!(!usages.contains(&"console".to_string()));
    }

    // ========== SVELTE ARROW FUNCTION TESTS ==========

    #[test]
    fn test_svelte_arrow_function_event_handlers() {
        let template = r#"
            <button on:click={() => save()}>Save</button>
            <button on:click={(e) => handleClick(e)}>Click</button>
            <input on:input={() => validate(value)}>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(
            usages.contains(&"save".to_string()),
            "Should detect 'save' from arrow function, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"handleClick".to_string()),
            "Should detect 'handleClick' from arrow function with param, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"validate".to_string()),
            "Should detect 'validate' from arrow function, found: {:?}",
            usages
        );
    }

    #[test]
    fn test_svelte_mixed_event_handlers() {
        let template = r#"
            <button on:click={directHandler}>Direct</button>
            <button on:click={() => arrowHandler()}>Arrow</button>
            <button on:click={(event) => complexHandler(event, data)}>Complex</button>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(usages.contains(&"directHandler".to_string()));
        assert!(usages.contains(&"arrowHandler".to_string()));
        assert!(usages.contains(&"complexHandler".to_string()));
    }

    #[test]
    fn test_svelte_component_method_calls() {
        // Test component method calls via bind:this pattern
        // Pattern: <ChatInput bind:this={chatInput} /> then chatInput?.focusInput()
        let template = r#"
            <ChatInput bind:this={chatInput} />
            <button on:click={() => chatInput?.focusInput()}>Focus</button>
            <div>{modal.show()}</div>
            <input use:action={() => input.getValue()}>
        "#;

        let usages = parse_svelte_template_usages(template);
        assert!(
            usages.contains(&"focusInput".to_string()),
            "Should detect focusInput method call via optional chaining, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"chatInput".to_string()),
            "Should detect chatInput object, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"show".to_string()),
            "Should detect show method call, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"modal".to_string()),
            "Should detect modal object, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"getValue".to_string()),
            "Should detect getValue method call, found: {:?}",
            usages
        );
        assert!(
            usages.contains(&"input".to_string()),
            "Should detect input object, found: {:?}",
            usages
        );
    }

    // ========== VUE TEMPLATE TESTS ==========

    #[test]
    fn test_vue_template_function_calls() {
        let template = r#"
            <div>
                <span>{{ formatDate(date) }}</span>
                <p>{{ computeTotal(items, tax) }}</p>
            </div>
        "#;

        let usages = parse_vue_template_usages(template);
        assert!(usages.contains(&"formatDate".to_string()));
        assert!(usages.contains(&"computeTotal".to_string()));
    }

    #[test]
    fn test_vue_template_event_handlers() {
        let template = r#"
            <button @click="handleClick">Click</button>
            <input @input="onInputChange" />
            <form v-on:submit="submitForm">...</form>
        "#;

        let usages = parse_vue_template_usages(template);
        assert!(usages.contains(&"handleClick".to_string()));
        assert!(usages.contains(&"onInputChange".to_string()));
        assert!(usages.contains(&"submitForm".to_string()));
    }

    #[test]
    fn test_vue_template_prop_bindings() {
        let template = r#"
            <Component :value="computedValue" :data="myData" />
            <div v-bind:class="dynamicClass">Content</div>
        "#;

        let usages = parse_vue_template_usages(template);
        assert!(usages.contains(&"computedValue".to_string()));
        assert!(usages.contains(&"myData".to_string()));
        assert!(usages.contains(&"dynamicClass".to_string()));
    }

    #[test]
    fn test_vue_template_v_model() {
        let template = r#"
            <input v-model="username" />
            <select v-model="selectedOption">...</select>
        "#;

        let usages = parse_vue_template_usages(template);
        assert!(usages.contains(&"username".to_string()));
        assert!(usages.contains(&"selectedOption".to_string()));
    }

    #[test]
    fn test_vue_template_components() {
        let template = r#"
            <MyComponent :prop="value" />
            <AnotherWidget />
            <div><NestedComponent /></div>
        "#;

        let usages = parse_vue_template_usages(template);
        assert!(usages.contains(&"MyComponent".to_string()));
        assert!(usages.contains(&"AnotherWidget".to_string()));
        assert!(usages.contains(&"NestedComponent".to_string()));
    }

    #[test]
    fn test_vue_builtins_not_detected() {
        let template = r#"
            <div v-if="condition">
                <component :is="dynamicComponent" />
                <transition name="fade">
                    <keep-alive>
                        <component />
                    </keep-alive>
                </transition>
            </div>
        "#;

        let usages = parse_vue_template_usages(template);
        assert!(!usages.contains(&"if".to_string()));
        assert!(!usages.contains(&"component".to_string()));
        assert!(!usages.contains(&"transition".to_string()));
        assert!(!usages.contains(&"console".to_string()));
    }
}
