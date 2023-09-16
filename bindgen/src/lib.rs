//#![feature(proc_macro_diagnostic)]

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use fluent_bundle::FluentResource;
use fluent_localization_loader::{
    base_path, fold_displayable, load_resources_from_folder, DEFAULT_DIR,
};
use fluent_syntax::ast::{Entry, Expression, InlineExpression, PatternElement};
use proc_macro::TokenStream;
use quote::quote;
use syn::LitStr;

//hardcode the alphabet, seems to be the fastest way to do this
const ALPHABET: [char; 26] = [
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];

struct Node<'a> {
    category: &'a str,
    name: &'a str,
    variables: HashSet<&'a str>,
    dependencies: HashSet<&'a str>,
    term: bool,
}

impl<'a> Node<'a> {
    pub fn new(category: &'a str, name: &'a str, term: bool) -> Self {
        Node {
            category,
            name,
            variables: HashSet::new(),
            dependencies: HashSet::new(),
            term,
        }
    }
}
#[proc_macro]
pub fn bind_localizations(_meta: TokenStream) -> TokenStream {
    //Load the bundle

    let mut base_dir = base_path();
    base_dir.push(DEFAULT_DIR);

    let resources = match load_resources_from_folder(base_dir) {
        Ok(value) => value,
        Err(e) => panic!("{}", fold_displayable(e.chain(), "| Caused by: ")),
    };

    // Walk each resource and generaate its nodes, then collect them all in a singular hashmap.
    //No need to worry about duplicates since that would have yieled a loading error earlyier on
    let mut nodes_map: HashMap<String, Node> = resources
        .iter()
        .flat_map(|resource| generate_nodes_for(&resource.name, &resource.resource))
        .map(|node| (node.name.to_string(), node))
        .collect();

    //Assemble full list for later, filter out terms cause we can't enforce their pressence sadly
    let all_terms: Vec<LitStr> = nodes_map
        .iter()
        .filter(|(_, node)| node.term)
        .map(|(name, _)| syn::LitStr::new(name.as_str(), proc_macro2::Span::call_site()))
        .collect();
    let all_messages: Vec<LitStr> = nodes_map
        .iter()
        .filter(|(_, node)| !node.term)
        .map(|(name, _)| syn::LitStr::new(name.as_str(), proc_macro2::Span::call_site()))
        .collect();
    //println!("{all_names:?}");

    //Nodes can depend on other nodes, copy over all the dependecies where needed
    // ! Recursion checking required in since fluent doesn't give parse errors on these so we need to avoid infinite loops here !
    loop {
        // rust mutability can be a pain in the ass sometimes so we have to do this the hard way
        let Some(todo) = nodes_map
            .iter()
            .filter_map(|(_, node)| node.dependencies.iter().next().map(|todo| todo.to_string()))
            .next()
        else {
            break;
        };

        let Some((variables, dependencies)) = nodes_map
            .get(todo.as_str())
            .map(|node| (node.variables.clone(), node.dependencies.clone()))
        else {
            panic!(
                "Enountered a dependency on localization node {todo} but no such node was loaded"
            );
        };

        for (name, node) in nodes_map
            .iter_mut()
            .filter(|(_, node)| node.dependencies.contains(todo.as_str()))
        {
            if name.as_str() == todo.as_str() {
                panic!("Cyclic localization loop detected at node {name}!");
            }

            node.dependencies.remove(todo.as_str());
            node.variables.extend(variables.iter());
            node.dependencies.extend(dependencies.iter());
        }
    }

    // General code for validating the bundle and handling errors

    let mut code = quote! {
        pub struct LanguageLocalizer<'a> {
            localizations: &'a fluent_localization_loader::LocalizationHolder,
            language: &'a str,
        }


        impl <'a> LanguageLocalizer<'a> {
            pub fn new(holder: &'a fluent_localization_loader::LocalizationHolder, language: &'a str) -> LanguageLocalizer<'a> {
                LanguageLocalizer {
                    localizations: holder,
                    language,
                }
            }


            pub fn validate_default_bundle_complete() -> anyhow::Result<()> {
                tracing::debug!("Validating default bundle has all expected keys");
                let expected_messages: Vec<&str> = vec![#(#all_messages,)*];
                let expected_terms: Vec<&str> = vec![#(#all_terms,)*];
                let mut base_dir = fluent_localization_loader::base_path();
                let default_lang = fluent_localization_loader::get_default_language()?;

                base_dir.push(default_lang.to_string());

                let resources = fluent_localization_loader::load_resources_from_folder(base_dir)?;

                let mut found_messages: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut found_terms: std::collections::HashSet<String> = std::collections::HashSet::new();

                resources.iter()
                .flat_map(|resource| resource.resource.entries())
                .for_each(|entry| {
                    match entry {
                        fluent_syntax::ast::Entry::Message(message) => {
                            if message.value.is_some()  {
                                found_messages.insert(message.id.name.to_string());
                            }
                        }
                        fluent_syntax::ast::Entry::Term(term) => {
                            found_terms.insert(term.id.name.to_string());
                        },
                        _ => ()
                }
            });

                let missing_messages: Vec<&str> = expected_messages.into_iter().filter(|name| !found_messages.contains(&name.to_string())).collect();
                let missing_terms: Vec<&str> = expected_terms.into_iter().filter(|name| !found_terms.contains(&name.to_string())).collect();
                if missing_messages.is_empty() && missing_terms.is_empty()  {
                    tracing::info!("Default bundle ({default_lang} is valid");
                    Ok(())
                } else {
                    Err(fluent_localization_loader::LocalizationLoadingError::new(format!("The following localization keys where not found in the default language bundle: {}", fluent_localization_loader::fold_displayable(missing_messages.into_iter().map(|name| name.to_string()).chain(missing_terms.into_iter().map(|name| format!("-{name}"))), ", "))))?
                }
            }

            pub fn localize(&self, name: &str, arguments: Option<fluent_bundle::FluentArgs<'a>>) -> String {
                let bundle = self.localizations.get_bundle(self.language);
                //This is autogenerated from the same list as the bundle validator so we know this is present
                let message = bundle.get_message(name).unwrap();

                let mut errors = Vec::new();

                let message = bundle.format_pattern(message.value().unwrap(), arguments.as_ref(), &mut errors);


                if errors.is_empty() {
                    message.to_string()
                } else {
                    self.handle_errors(name, errors)
                }

            }

            pub fn handle_errors(&self, name: &str, errors: Vec<fluent_bundle::FluentError>) -> String {
                let errors = fluent_localization_loader::fold_displayable(errors.into_iter(), ", ");
                tracing::error!("Failed to localize {name} due to following errors: {errors}");

                //TODO: actually report this error somewhere other then logs?
                format!("Failed to localize the \"{name}\" response.")
            }




        }
    };

    //Now let's generate the helper functions, just from strings now cause that's easier with all the damn generics

    // let's start easy: no params here
    let start = String::from("impl <'a> LanguageLocalizer<'a> {");
    let mut simple_block = nodes_map
        .iter()
        .filter(|(_, node)| node.variables.is_empty() && !node.term)
        .map(|(name, node)| {
            let category = sanitize(node.category);
            let sanitized_name = sanitize(name);
            format!(
                "
\tpub fn {category}_{sanitized_name}(&self) -> String {{
\t\tself.localize(\"{name}\", None)
\t}}"
            )
        })
        .fold(start, |assembled, extra| assembled + "\n" + &extra);
    simple_block += "\n}";
    //println!("{simple_block}");
    let compiled_simple_block = simple_block
        .parse::<proc_macro2::TokenStream>()
        .expect("Failed to assemble simple block token stream");

    code.extend(compiled_simple_block);

    //Now it gets real, welcome to generated generics
    let hell = nodes_map
        .iter()
        .filter(|(_, node)| !node.variables.is_empty() && !node.term)
        .map(|(name, node)| {
            let count = node.variables.len();
            let letters = get_letters(count);

            let generics = format!("<{}>", fold_displayable(letters.iter(), ", "));

            let generic_definitions = letters
                .iter()
                .map(|letter| format!("\t{letter}: Into<fluent_bundle::FluentValue<'a>>,"))
                .fold(String::from("where"), |assembled, extra| {
                    assembled + "\n" + &extra
                })
                + "\n";

            let mut letter_iter = letters.iter();
            let mut params = String::from("&self");
            let mut handle_arguments =
                String::from("let mut arguments = fluent_bundle::FluentArgs::new();");
            for name in &node.variables {
                let sanitized_name = sanitize(name);
                // safe to unwrap, we generated the letters based on the variable count above
                let letter = letter_iter.next().unwrap();

                params += &format!(", {sanitized_name}: {letter}");
                handle_arguments +=
                    &format!("\n\t\targuments.set(\"{name}\", {sanitized_name}.into());");
            }

            let category = node.category;
            let sanitized_name = sanitize(name);
            format!(
                "
\tpub fn {category}_{sanitized_name}{generics}({params}) -> String
\t{generic_definitions}\t{{
\t\t{handle_arguments}
\t\tself.localize(\"{name}\", Some(arguments))
\t}}"
            )
        })
        .fold(
            String::from("impl <'a> LanguageLocalizer<'a> {"),
            |assembled, extra| assembled + "\n" + &extra,
        )
        + "\n}";

    //println!("{hell}");
    let compiled_hell_block = hell
        .parse::<proc_macro2::TokenStream>()
        .expect("Failed to assemble the token stream from hell");

    code.extend(compiled_hell_block);

    code.into()
}

fn sanitize(original: &str) -> String {
    original.replace('-', "_").to_lowercase()
}

fn get_letters(amount: usize) -> Vec<char> {
    if amount > 26 {
        todo!("Localization strings with 26+ params, what the hell is this? are we assembling a phone book?");
    }
    (0..amount).map(|count| ALPHABET[count]).collect()
}

fn generate_nodes_for<'a>(parrent: &'a str, resource: &'a Arc<FluentResource>) -> Vec<Node<'a>> {
    let mut out = Vec::new();

    for entry in resource.entries() {
        let (name, pattern, term) = match entry {
            Entry::Message(message) => {
                let Some(pattern) = &message.value else {
                    continue;
                };
                (message.id.name, pattern, false)
            }
            Entry::Term(term) => (term.id.name, &term.value, true),
            _ => continue,
        };

        let mut node = Node::new(parrent, name, term);
        process_pattern_elements(&pattern.elements, &mut node);
        out.push(node)
    }

    out
}

fn process_pattern_elements<'a>(attributes: &'a Vec<PatternElement<&'a str>>, node: &mut Node<'a>) {
    for attribute in attributes {
        // We only care about placables since those are dynamic, we are not interested in fixed textelements
        match attribute {
            PatternElement::TextElement { value: _ } => (),
            PatternElement::Placeable { expression } => {
                process_expression(expression, node);
            }
        }
    }
}

fn process_expression<'a>(expression: &'a Expression<&'a str>, node: &mut Node<'a>) {
    match expression {
        Expression::Select { selector, variants } => {
            process_inline_expression(selector, node);
            for variant in variants {
                process_pattern_elements(&variant.value.elements, node)
            }
        }
        Expression::Inline(inline) => process_inline_expression(inline, node),
    }
}

fn process_inline_expression<'a>(expression: &'a InlineExpression<&'a str>, node: &mut Node<'a>) {
    match expression {
        InlineExpression::FunctionReference {
            id: _,
            arguments: _,
        } => todo!(), // leaving this as a crash intentionally for now since i don't know how these work exactly yet. will deal with this if i ever end up actually using them
        InlineExpression::MessageReference { id, attribute: _ } => {
            node.dependencies.insert(id.name);
        }
        InlineExpression::TermReference {
            id,
            attribute: _,
            arguments: _,
        } => {
            node.dependencies.insert(id.name);
        }
        InlineExpression::VariableReference { id } => {
            node.variables.insert(id.name);
        }
        InlineExpression::Placeable { expression } => {
            process_expression(expression, node);
        }
        InlineExpression::StringLiteral { value: _ }
        | InlineExpression::NumberLiteral { value: _ } => {}
    }

    if let InlineExpression::VariableReference { id } = expression {
        node.variables.insert(id.name);
    }
}
