use std::collections::BTreeMap;
use std::mem;
use yaml_rust::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust::scanner::{Marker, TScalarStyle, TokenType};
use yaml_rust::yaml::{Array, Hash, Yaml};

use crate::jless::flatjson::{ContainerType, Index, OptionIndex, Row, Value};

const MAX_EXPANDED_YAML_NODES: usize = 100_000;

struct YamlParser {
    parents: Vec<Index>,
    rows: Vec<Row>,
    pretty_printed: String,
    max_depth: usize,
}

pub fn parse(yaml: String) -> Result<(Vec<Row>, String, usize), String> {
    let mut event_parser = Parser::new(yaml.chars());
    let mut loader = BudgetedYamlLoader::default();
    let load_result = event_parser.load(&mut loader, true);
    if let Some(error) = loader.error {
        return Err(error);
    }
    load_result.map_err(|error| error.to_string())?;

    let mut parser = YamlParser {
        parents: vec![],
        rows: vec![],
        pretty_printed: String::new(),
        max_depth: 0,
    };

    let mut prev_sibling = OptionIndex::Nil;

    for (i, doc) in loader.docs.into_iter().enumerate() {
        if i != 0 {
            parser.pretty_printed.push('\n');
        }
        let index = parser.parse_yaml_item(doc)?;

        parser.rows[index].prev_sibling = prev_sibling;
        parser.rows[index].index_in_parent = i;
        if let OptionIndex::Index(prev) = prev_sibling {
            parser.rows[prev].next_sibling = OptionIndex::Index(index);
        }

        prev_sibling = OptionIndex::Index(index);
    }

    Ok((parser.rows, parser.pretty_printed, parser.max_depth))
}

#[derive(Default)]
struct BudgetedYamlLoader {
    docs: Vec<Yaml>,
    doc_stack: Vec<(Yaml, usize, usize)>,
    key_stack: Vec<Yaml>,
    anchor_map: BTreeMap<usize, (Yaml, usize)>,
    expanded_nodes: usize,
    error: Option<String>,
}

impl BudgetedYamlLoader {
    fn count_nodes(&mut self, count: usize) -> bool {
        if self.expanded_nodes.saturating_add(count) > MAX_EXPANDED_YAML_NODES {
            self.error = Some(format!(
                "YAML preview exceeds the {MAX_EXPANDED_YAML_NODES} expanded node limit"
            ));
            return false;
        }
        self.expanded_nodes += count;
        true
    }

    fn insert_new_node(&mut self, node: Yaml, anchor_id: usize, expanded_size: usize) {
        if anchor_id > 0 {
            self.anchor_map
                .insert(anchor_id, (node.clone(), expanded_size));
        }

        if self.doc_stack.is_empty() {
            self.doc_stack.push((node, anchor_id, expanded_size));
            return;
        }

        let parent = self.doc_stack.last_mut().unwrap();
        match parent.0 {
            Yaml::Array(ref mut array) => array.push(node),
            Yaml::Hash(ref mut hash) => {
                let current_key = self.key_stack.last_mut().unwrap();
                if current_key.is_badvalue() {
                    *current_key = node;
                } else {
                    let mut key = Yaml::BadValue;
                    mem::swap(&mut key, current_key);
                    hash.insert(key, node);
                }
            }
            _ => unreachable!(),
        }
        parent.2 += expanded_size;
    }

    fn finish_collection(&mut self) {
        if let Some((node, anchor_id, expanded_size)) = self.doc_stack.pop() {
            self.insert_new_node(node, anchor_id, expanded_size);
        }
    }
}

impl MarkedEventReceiver for BudgetedYamlLoader {
    fn on_event(&mut self, event: Event, _: Marker) {
        if self.error.is_some() {
            return;
        }

        match event {
            Event::DocumentStart => self.anchor_map.clear(),
            Event::DocumentEnd => match self.doc_stack.len() {
                0 => self.docs.push(Yaml::BadValue),
                1 => self.docs.push(self.doc_stack.pop().unwrap().0),
                _ => unreachable!("YAML document ended with open collections"),
            },
            Event::SequenceStart(anchor_id) => {
                if self.count_nodes(1) {
                    self.doc_stack
                        .push((Yaml::Array(Array::new()), anchor_id, 1));
                }
            }
            Event::SequenceEnd => self.finish_collection(),
            Event::MappingStart(anchor_id) => {
                if self.count_nodes(1) {
                    self.doc_stack.push((Yaml::Hash(Hash::new()), anchor_id, 1));
                    self.key_stack.push(Yaml::BadValue);
                }
            }
            Event::MappingEnd => {
                self.key_stack.pop();
                self.finish_collection();
            }
            Event::Scalar(value, style, anchor_id, tag) => {
                if self.count_nodes(1) {
                    self.insert_new_node(scalar_to_yaml(value, style, tag), anchor_id, 1);
                }
            }
            Event::Alias(anchor_id) => {
                if let Some(expanded_size) = self
                    .anchor_map
                    .get(&anchor_id)
                    .map(|(_, expanded_size)| *expanded_size)
                {
                    if self.count_nodes(expanded_size) {
                        let node = self.anchor_map.get(&anchor_id).unwrap().0.clone();
                        self.insert_new_node(node, 0, expanded_size);
                    }
                } else if self.count_nodes(1) {
                    self.insert_new_node(Yaml::BadValue, 0, 1);
                }
            }
            _ => {}
        }
    }
}

fn scalar_to_yaml(value: String, style: TScalarStyle, tag: Option<TokenType>) -> Yaml {
    if style != TScalarStyle::Plain {
        return Yaml::String(value);
    }

    if let Some(TokenType::Tag(ref handle, ref suffix)) = tag {
        if handle == "!!" {
            return match suffix.as_ref() {
                "bool" => value
                    .parse::<bool>()
                    .map(Yaml::Boolean)
                    .unwrap_or(Yaml::BadValue),
                "int" => value
                    .parse::<i64>()
                    .map(Yaml::Integer)
                    .unwrap_or(Yaml::BadValue),
                "float" if is_yaml_float(&value) => Yaml::Real(value),
                "float" => Yaml::BadValue,
                "null" if matches!(value.as_str(), "~" | "null") => Yaml::Null,
                "null" => Yaml::BadValue,
                _ => Yaml::String(value),
            };
        }
        return Yaml::String(value);
    }

    Yaml::from_str(&value)
}

fn is_yaml_float(value: &str) -> bool {
    matches!(
        value,
        ".inf"
            | ".Inf"
            | ".INF"
            | "+.inf"
            | "+.Inf"
            | "+.INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | "NaN"
            | ".NAN"
    ) || value.parse::<f64>().is_ok()
}

impl YamlParser {
    fn parse_yaml_item(&mut self, item: Yaml) -> Result<usize, String> {
        self.max_depth = self.max_depth.max(self.parents.len());

        let index = match item {
            Yaml::BadValue => return Err("Unknown YAML parse error".to_owned()),
            Yaml::Null => self.parse_null(),
            Yaml::Boolean(b) => self.parse_bool(b),
            Yaml::Integer(i) => self.parse_number(i.to_string()),
            Yaml::Real(real_str) => self.parse_number(real_str),
            Yaml::String(s) => self.parse_string(s),
            Yaml::Array(arr) => self.parse_array(arr)?,
            Yaml::Hash(hash) => self.parse_hash(hash)?,
            // The yaml_rust source says these are not supported yet.
            // Aliases are automatically replaced by their anchors, so
            // it's unclear what this would be used for.
            Yaml::Alias(_) => return Err("YAML parser returned Alias value".to_owned()),
        };

        Ok(index)
    }

    fn parse_null(&mut self) -> usize {
        let row_index = self.create_row(Value::Null);
        self.rows[row_index].range.end = self.rows[row_index].range.start + 4;
        self.pretty_printed.push_str("null");
        row_index
    }

    fn parse_bool(&mut self, b: bool) -> usize {
        let row_index = self.create_row(Value::Boolean);
        let (bool_str, len) = if b { ("true", 4) } else { ("false", 5) };

        self.rows[row_index].range.end = self.rows[row_index].range.start + len;
        self.pretty_printed.push_str(bool_str);

        row_index
    }

    fn parse_number(&mut self, num_s: String) -> usize {
        let row_index = self.create_row(Value::Number);
        self.pretty_printed.push_str(&num_s);

        self.rows[row_index].range.end = self.rows[row_index].range.start + num_s.len();

        row_index
    }

    fn parse_string(&mut self, s: String) -> usize {
        let row_index = self.create_row(Value::String);

        // Escape newlines.
        let s = s.replace('\n', "\\n");

        self.pretty_printed.push('"');
        self.pretty_printed.push_str(&s);
        self.pretty_printed.push('"');
        self.rows[row_index].range.end = self.rows[row_index].range.start + s.len() + 2;

        row_index
    }

    fn parse_array(&mut self, arr: Array) -> Result<usize, String> {
        if arr.is_empty() {
            let row_index = self.create_row(Value::EmptyArray);
            self.rows[row_index].range.end = self.rows[row_index].range.start + 2;
            self.pretty_printed.push_str("[]");
            return Ok(row_index);
        }

        let open_value = Value::OpenContainer {
            container_type: ContainerType::Array,
            collapsed: false,
            // To be set when parsing is complete.
            first_child: 0,
            close_index: 0,
        };

        let array_open_index = self.create_row(open_value);

        self.parents.push(array_open_index);
        self.pretty_printed.push('[');

        let mut prev_sibling = OptionIndex::Nil;

        for (i, child) in arr.into_iter().enumerate() {
            if i != 0 {
                self.pretty_printed.push_str(", ");
            }

            let child_index = self.parse_yaml_item(child)?;

            if i == 0 {
                match self.rows[array_open_index].value {
                    Value::OpenContainer {
                        ref mut first_child,
                        ..
                    } => {
                        *first_child = child_index;
                    }
                    _ => panic!("Must be Array!"),
                }
            }

            self.rows[child_index].prev_sibling = prev_sibling;
            self.rows[child_index].index_in_parent = i;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child_index);
            }

            prev_sibling = OptionIndex::Index(child_index);
        }

        self.parents.pop();

        let close_value = Value::CloseContainer {
            container_type: ContainerType::Array,
            collapsed: false,
            last_child: prev_sibling.unwrap(),
            open_index: array_open_index,
        };

        let array_close_index = self.create_row(close_value);

        // Update end of the Array range; we add the ']' to pretty_printed
        // below, hence the + 1.
        self.rows[array_open_index].range.end = self.pretty_printed.len() + 1;

        match self.rows[array_open_index].value {
            Value::OpenContainer {
                ref mut close_index,
                ..
            } => {
                *close_index = array_close_index;
            }
            _ => panic!("Must be Array!"),
        }

        self.pretty_printed.push(']');
        Ok(array_open_index)
    }

    fn parse_hash(&mut self, hash: Hash) -> Result<usize, String> {
        if hash.is_empty() {
            let row_index = self.create_row(Value::EmptyObject);
            self.rows[row_index].range.end = self.rows[row_index].range.start + 2;
            self.pretty_printed.push_str("{}");
            return Ok(row_index);
        }

        let open_value = Value::OpenContainer {
            container_type: ContainerType::Object,
            collapsed: false,
            // To be set when parsing is complete.
            first_child: 0,
            close_index: 0,
        };

        let object_open_index = self.create_row(open_value);

        self.parents.push(object_open_index);
        self.pretty_printed.push('{');

        let mut prev_sibling = OptionIndex::Nil;

        for (i, (key, value)) in hash.into_iter().enumerate() {
            if i == 0 {
                // Add space inside objects.
                self.pretty_printed.push(' ');
            } else {
                self.pretty_printed.push_str(", ");
            }

            /////////////////////////////////

            let key_range = {
                let key_range_start = self.pretty_printed.len();

                self.pretty_print_key_item(key, true)?;

                let key_range_end = self.pretty_printed.len();

                key_range_start..key_range_end
            };

            self.pretty_printed.push_str(": ");

            let child_index = self.parse_yaml_item(value)?;

            self.rows[child_index].key_range = Some(key_range);

            if i == 0 {
                match self.rows[object_open_index].value {
                    Value::OpenContainer {
                        ref mut first_child,
                        ..
                    } => {
                        *first_child = child_index;
                    }
                    _ => panic!("Must be Object!"),
                }
            }

            self.rows[child_index].prev_sibling = prev_sibling;
            self.rows[child_index].index_in_parent = i;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child_index);
            }

            prev_sibling = OptionIndex::Index(child_index);
        }

        self.parents.pop();

        // Print space inside closing brace.
        self.pretty_printed.push(' ');

        let close_value = Value::CloseContainer {
            container_type: ContainerType::Object,
            collapsed: false,
            last_child: prev_sibling.unwrap(),
            open_index: object_open_index,
        };

        let object_close_index = self.create_row(close_value);

        // Update end of the Object range; we add the '}' to pretty_printed
        // below, hence the + 1.
        self.rows[object_open_index].range.end = self.pretty_printed.len() + 1;

        match self.rows[object_open_index].value {
            Value::OpenContainer {
                ref mut close_index,
                ..
            } => {
                *close_index = object_close_index;
            }
            _ => panic!("Must be Object!"),
        }

        self.pretty_printed.push('}');
        Ok(object_open_index)
    }

    fn pretty_print_key_item(&mut self, item: Yaml, is_key: bool) -> Result<(), String> {
        if let Yaml::String(s) = item {
            // Replace newlines.
            let s = s.replace('\n', "\\n");
            self.pretty_printed.push('"');
            self.pretty_printed.push_str(&s);
            self.pretty_printed.push('"');
            return Ok(());
        }

        if is_key {
            self.pretty_printed.push('[');
        }

        match item {
            Yaml::BadValue => return Err("Unknown YAML parse error".to_owned()),
            Yaml::Null => self.pretty_printed.push_str("null"),
            Yaml::Boolean(b) => self
                .pretty_printed
                .push_str(if b { "true" } else { "false " }),
            Yaml::Integer(i) => self.pretty_printed.push_str(&i.to_string()),
            Yaml::Real(real_str) => self.pretty_printed.push_str(&real_str),
            Yaml::Array(arr) => {
                if arr.is_empty() {
                    self.pretty_printed.push_str("[]");
                } else {
                    self.pretty_printed.push('[');
                    for (i, elem) in arr.into_iter().enumerate() {
                        if i != 0 {
                            self.pretty_printed.push_str(", ");
                        }
                        self.pretty_print_key_item(elem, false)?;
                    }
                    self.pretty_printed.push(']');
                }
            }
            Yaml::Hash(hash) => {
                if hash.is_empty() {
                    self.pretty_printed.push_str("{}");
                } else {
                    self.pretty_printed.push_str("{ ");
                    for (i, (key, value)) in hash.into_iter().enumerate() {
                        if i != 0 {
                            self.pretty_printed.push_str(", ");
                        }
                        self.pretty_print_key_item(key, true)?;
                        self.pretty_printed.push_str(": ");
                        self.pretty_print_key_item(value, false)?;
                    }
                    self.pretty_printed.push_str(" }");
                }
            }
            // The yaml_rust source says these are not supported yet.
            // Aliases are automatically replaced by their anchors, so
            // it's unclear what this would be used for.
            Yaml::Alias(_) => return Err("YAML parser returned Alias value".to_owned()),
            Yaml::String(_) => unreachable!(),
        }

        if is_key {
            self.pretty_printed.push(']');
        }

        Ok(())
    }

    // Add a new row to the FlatJson representation.
    //
    // self.pretty_printed should NOT include the added row yet;
    // we use the current length of self.pretty_printed as the
    // starting index of the row's range.
    fn create_row(&mut self, value: Value) -> usize {
        let index = self.rows.len();

        let parent = match self.parents.last() {
            None => OptionIndex::Nil,
            Some(row_index) => OptionIndex::Index(*row_index),
        };

        let range_start = self.pretty_printed.len();

        self.rows.push(Row {
            // Set correctly by us
            parent,
            depth: self.parents.len(),
            value,

            // The start of this range is set by us, but then we set
            // the end when we're done parsing the row. We'll set
            // the default end to be one character so we don't have to
            // update it after ']' and '}'.
            range: range_start..range_start + 1,

            // To be filled in by caller
            prev_sibling: OptionIndex::Nil,
            next_sibling: OptionIndex::Nil,
            index_in_parent: 0,
            key_range: None,
        });

        index
    }
}

#[cfg(all(test, any()))]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_basic() {
        // 0 2    7  10   15    21   26    32     39 42
        // { "a": 1, "b": true, "c": null, "ddd": [] }
        let yaml = indoc! {r#"
            ---
            a: 1
            b: true
            c: null
            ddd: []
        "#}
        .to_owned();
        let (rows, _, _) = parse(yaml).unwrap();

        assert_eq!(rows[0].range, 0..43); // Object
        assert_eq!(rows[1].key_range, Some(2..5)); // "a": 1
        assert_eq!(rows[1].range, 7..8); // "a": 1
        assert_eq!(rows[2].key_range, Some(10..13)); // "b": true
        assert_eq!(rows[2].range, 15..19); // "b": true
        assert_eq!(rows[3].key_range, Some(21..24)); // "c": null
        assert_eq!(rows[3].range, 26..30); // "c": null
        assert_eq!(rows[4].range, 39..41); // "ddd": []
        assert_eq!(rows[5].range, 42..43); // }

        // 01   5        14     21 23
        // [14, "apple", false, {}]
        let yaml = indoc! {r#"
            ---
            - 14
            - apple
            - false
            - {}
        "#}
        .to_owned();
        let (rows, _, _) = parse(yaml).unwrap();

        assert_eq!(rows[0].range, 0..24); // Array
        assert_eq!(rows[1].range, 1..3); // 14
        assert_eq!(rows[2].range, 5..12); // "apple"
        assert_eq!(rows[3].range, 14..19); // false
        assert_eq!(rows[4].range, 21..23); // {}
        assert_eq!(rows[5].range, 23..24); // ]

        // 01 3      10     17    23  27   32   37 40    46   51
        // [{ "abc": "str", "de": 14, "f": null }, true, false]
        let yaml = indoc! {r#"
            ---
            - abc: str
              de: 14
              f: null
            - true
            - false
        "#}
        .to_owned();
        let (rows, _, _) = parse(yaml).unwrap();

        assert_eq!(rows[0].range, 0..52); // Array
        assert_eq!(rows[1].range, 1..38); // Object
        assert_eq!(rows[2].key_range, Some(3..8)); // "abc": "str"
        assert_eq!(rows[2].range, 10..15); // "abc": "str"
        assert_eq!(rows[3].key_range, Some(17..21)); // "de": 14
        assert_eq!(rows[3].range, 23..25); // "de": 14
        assert_eq!(rows[4].key_range, Some(27..30)); // "f": null
        assert_eq!(rows[4].range, 32..36); // "f": null
        assert_eq!(rows[5].range, 37..38); // }
        assert_eq!(rows[6].range, 40..44); // true
        assert_eq!(rows[7].range, 46..51); // false
        assert_eq!(rows[8].range, 51..52); // ]
    }

    #[test]
    fn test_non_scalar_keys() {
        let yaml = indoc! {r#"
            ---
            [1, 2]: 1
            { a: 1, b: 2 }: true
        "#}
        .to_owned();
        //              0 2       1012 15                  3537   42
        let pretty = r#"{ [[1, 2]]: 1, [{ "a": 1, "b": 2 }]: true }"#;
        let (rows, parsed_pretty, _) = parse(yaml).unwrap();

        assert_eq!(pretty, parsed_pretty);

        assert_eq!(rows[0].range, 0..43); // Object
        assert_eq!(rows[1].key_range, Some(2..10)); // [[1, 2]]
        assert_eq!(rows[1].range, 12..13); // [[1, 2]]: 1
        assert_eq!(rows[2].key_range, Some(15..35)); // [{ "a": 1, "b": 2 }]
        assert_eq!(rows[2].range, 37..41); // [{ "a": 1, "b": 2 }]: true
    }

    #[test]
    fn test_multiline_strings() {
        let yaml = indoc! {r#"
            ---
            str1:
              fl
              ow
            str2: |
                a
                b
            str3: >
                fol
                ded
            ? |
                key
                string
            : 1
        "#}
        .to_owned();
        let pretty =
            r#"{ "str1": "fl ow", "str2": "a\nb\n", "str3": "fol ded\n", "key\nstring\n": 1 }"#;
        let (_, parsed_pretty, _) = parse(yaml).unwrap();

        assert_eq!(pretty, parsed_pretty);
    }
}
