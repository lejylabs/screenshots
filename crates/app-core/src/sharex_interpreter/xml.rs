//! Bounded XML: DTD-free parsing, a single-node XPath 1.0 subset and body credential scanning.
use super::{Fail, R, limits};
use roxmltree::{Document, Node, NodeId, NodeType};

pub(super) fn parse(text: &str) -> R<Document<'_>> {
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 1 << 20, ..Default::default() };
    let document = Document::parse_with_options(text, options).map_err(|_| Fail::invalid())?;
    Ok(document)
}

/// XmlReader depth: the root element is 0 and its content is 1.
pub(super) fn require_depth(document: &Document, code: &'static str) -> R<()> {
    fn walk(node: Node, depth: usize, code: &'static str) -> R<()> {
        limits::require(depth <= 64, code)?;
        for child in node.children() {
            walk(child, depth + 1, code)?;
        }
        Ok(())
    }
    for child in document.root().children() {
        walk(child, 0, code)?;
    }
    Ok(())
}

/// XPathDocument drops whitespace-only text nodes.
fn visible(node: &Node) -> bool {
    match node.node_type() {
        NodeType::Text => !node.text().unwrap_or_default().trim().is_empty(),
        _ => true,
    }
}

pub(super) fn string_value(node: Node) -> String {
    match node.node_type() {
        NodeType::Text | NodeType::Comment | NodeType::PI => node.text().unwrap_or_default().to_owned(),
        _ => node
            .descendants()
            .filter(|n| n.is_text() && visible(n))
            .filter_map(|n| n.text())
            .collect(),
    }
}

// ---- XPath subset ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Node(NodeId),
    Attr(NodeId, usize),
}
impl Item {
    /// Document order: roxmltree stores nodes in document order; attributes follow their element.
    fn order(self) -> (usize, usize) {
        match self {
            Item::Node(id) => (id.get_usize(), 0),
            Item::Attr(id, index) => (id.get_usize(), index + 1),
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Itself,
    Parent,
    Attribute,
    Ancestor,
    FollowingSibling,
    PrecedingSibling,
}
enum Test {
    Name(String),
    Any,
    Text,
    Node,
    Comment,
}
struct Step {
    axis: Axis,
    test: Test,
    predicates: Vec<Pred>,
}
struct Path {
    absolute: bool,
    steps: Vec<Step>,
}
enum Pred {
    Or(Box<Pred>, Box<Pred>),
    And(Box<Pred>, Box<Pred>),
    Cmp(Box<Pred>, &'static str, Box<Pred>),
    Path(Path),
    Str(String),
    Num(f64),
    Call(String, Vec<Pred>),
}
enum Val {
    Nodes(Vec<Item>),
    Str(String),
    Num(f64),
    Bool(bool),
}

struct P<'a> {
    s: &'a [char],
    i: usize,
}
impl P<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }
    fn starts(&self, text: &str) -> bool {
        text.chars().enumerate().all(|(n, ch)| self.s.get(self.i + n) == Some(&ch))
    }
    fn eat(&mut self, text: &str) -> bool {
        if self.starts(text) {
            self.i += text.chars().count();
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> Option<String> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') && self.i > start)
        {
            if self.peek() == Some(':') && self.s.get(self.i + 1) == Some(&':') {
                break;
            }
            self.i += 1;
        }
        (self.i > start).then(|| self.s[start..self.i].iter().collect())
    }
    fn path(&mut self) -> R<Path> {
        self.ws();
        let mut steps = Vec::new();
        let absolute = self.peek() == Some('/');
        if absolute {
            if self.eat("//") {
                steps.push(Step { axis: Axis::DescendantOrSelf, test: Test::Node, predicates: vec![] });
            } else {
                self.i += 1;
                self.ws();
                if !self.peek().is_some_and(|ch| ch.is_alphabetic() || matches!(ch, '*' | '@' | '.' | '_')) {
                    return Ok(Path { absolute, steps });
                }
            }
        }
        loop {
            steps.push(self.step()?);
            self.ws();
            if self.eat("//") {
                steps.push(Step { axis: Axis::DescendantOrSelf, test: Test::Node, predicates: vec![] });
            } else if !self.eat("/") {
                break;
            }
        }
        Ok(Path { absolute, steps })
    }
    fn step(&mut self) -> R<Step> {
        self.ws();
        if self.eat("..") {
            return Ok(Step { axis: Axis::Parent, test: Test::Node, predicates: vec![] });
        }
        if self.eat(".") {
            return Ok(Step { axis: Axis::Itself, test: Test::Node, predicates: vec![] });
        }
        let mut axis = Axis::Child;
        if self.eat("@") {
            axis = Axis::Attribute;
        } else {
            let save = self.i;
            if let Some(name) = self.name()
                && self.eat("::")
            {
                axis = match name.as_str() {
                    "child" => Axis::Child,
                    "descendant" => Axis::Descendant,
                    "descendant-or-self" => Axis::DescendantOrSelf,
                    "self" => Axis::Itself,
                    "parent" => Axis::Parent,
                    "attribute" => Axis::Attribute,
                    "ancestor" => Axis::Ancestor,
                    "following-sibling" => Axis::FollowingSibling,
                    "preceding-sibling" => Axis::PrecedingSibling,
                    _ => return Err(Fail::invalid()),
                };
            } else {
                self.i = save;
            }
        }
        let test = if self.eat("*") {
            Test::Any
        } else if self.eat("text()") {
            Test::Text
        } else if self.eat("node()") {
            Test::Node
        } else if self.eat("comment()") {
            Test::Comment
        } else {
            let name = self.name().ok_or_else(Fail::invalid)?;
            // Prefixed names need a namespace manager that SelectSingleNode never receives.
            limits::require(!name.contains(':'), "invalid_expression_or_request")?;
            Test::Name(name)
        };
        let mut predicates = Vec::new();
        loop {
            self.ws();
            if !self.eat("[") {
                break;
            }
            predicates.push(self.or()?);
            self.ws();
            if !self.eat("]") {
                return Err(Fail::invalid());
            }
        }
        Ok(Step { axis, test, predicates })
    }
    fn or(&mut self) -> R<Pred> {
        let mut left = self.and()?;
        loop {
            self.ws();
            if self.eat("or ") {
                left = Pred::Or(Box::new(left), Box::new(self.and()?));
            } else {
                return Ok(left);
            }
        }
    }
    fn and(&mut self) -> R<Pred> {
        let mut left = self.cmp()?;
        loop {
            self.ws();
            if self.eat("and ") {
                left = Pred::And(Box::new(left), Box::new(self.cmp()?));
            } else {
                return Ok(left);
            }
        }
    }
    fn cmp(&mut self) -> R<Pred> {
        let left = self.primary()?;
        self.ws();
        for op in ["!=", "<=", ">=", "=", "<", ">"] {
            if self.eat(op) {
                let right = self.primary()?;
                return Ok(Pred::Cmp(Box::new(left), op, Box::new(right)));
            }
        }
        Ok(left)
    }
    fn primary(&mut self) -> R<Pred> {
        self.ws();
        match self.peek().ok_or_else(Fail::invalid)? {
            '(' => {
                self.i += 1;
                let inner = self.or()?;
                self.ws();
                if !self.eat(")") {
                    return Err(Fail::invalid());
                }
                Ok(inner)
            }
            quote @ ('\'' | '"') => {
                self.i += 1;
                let start = self.i;
                while self.peek().is_some_and(|ch| ch != quote) {
                    self.i += 1;
                }
                let text = self.s[start..self.i].iter().collect();
                if !self.eat(&quote.to_string()) {
                    return Err(Fail::invalid());
                }
                Ok(Pred::Str(text))
            }
            ch if ch.is_ascii_digit() || ch == '-' => {
                let start = self.i;
                self.i += 1;
                while self.peek().is_some_and(|ch| ch.is_ascii_digit() || ch == '.') {
                    self.i += 1;
                }
                let text: String = self.s[start..self.i].iter().collect();
                text.parse().map(Pred::Num).map_err(|_| Fail::invalid())
            }
            _ => {
                let save = self.i;
                if let Some(name) = self.name() {
                    self.ws();
                    if self.peek() == Some('(') && !matches!(name.as_str(), "text" | "node" | "comment") {
                        self.i += 1;
                        let mut args = Vec::new();
                        self.ws();
                        if !self.eat(")") {
                            loop {
                                args.push(self.or()?);
                                self.ws();
                                if self.eat(")") {
                                    break;
                                }
                                if !self.eat(",") {
                                    return Err(Fail::invalid());
                                }
                            }
                        }
                        return Ok(Pred::Call(name, args));
                    }
                }
                self.i = save;
                Ok(Pred::Path(self.path()?))
            }
        }
    }
}

struct Ctx<'a, 'input> {
    doc: &'a Document<'input>,
}
impl<'a, 'input> Ctx<'a, 'input> {
    fn node(&self, id: NodeId) -> Node<'a, 'input> {
        self.doc.get_node(id).expect("node id from this document")
    }
    fn matches(&self, item: Item, axis: Axis, test: &Test) -> bool {
        match item {
            Item::Attr(id, index) => {
                let attribute = self.node(id).attributes().nth(index);
                match (test, attribute) {
                    (Test::Any | Test::Node, Some(a)) => a.namespace().is_none() || matches!(test, Test::Node),
                    (Test::Name(name), Some(a)) => a.namespace().is_none() && a.name() == name,
                    _ => false,
                }
            }
            Item::Node(id) => {
                let node = self.node(id);
                match test {
                    Test::Node => true,
                    Test::Text => node.is_text(),
                    Test::Comment => node.is_comment(),
                    Test::Any => node.is_element() && axis != Axis::Attribute,
                    Test::Name(name) => {
                        node.is_element() && node.tag_name().namespace().is_none() && node.tag_name().name() == name
                    }
                }
            }
        }
    }
    fn axis(&self, item: Item, axis: Axis) -> Vec<Item> {
        let Item::Node(id) = item else {
            return match (axis, item) {
                (Axis::Parent | Axis::Ancestor, Item::Attr(id, _)) => {
                    let mut out = vec![Item::Node(id)];
                    if axis == Axis::Ancestor {
                        out.extend(self.node(id).ancestors().skip(1).map(|n| Item::Node(n.id())));
                    }
                    out
                }
                (Axis::Itself | Axis::DescendantOrSelf, _) => vec![item],
                _ => Vec::new(),
            };
        };
        let node = self.node(id);
        let wrap = |n: Node| Item::Node(n.id());
        match axis {
            Axis::Child => node.children().filter(visible).map(wrap).collect(),
            Axis::Descendant => node.descendants().skip(1).filter(visible).map(wrap).collect(),
            Axis::DescendantOrSelf => node.descendants().filter(visible).map(wrap).collect(),
            Axis::Itself => vec![item],
            Axis::Parent => node.parent().map(wrap).into_iter().collect(),
            Axis::Ancestor => node.ancestors().skip(1).map(wrap).collect(),
            Axis::FollowingSibling => node.next_siblings().skip(1).filter(visible).map(wrap).collect(),
            Axis::PrecedingSibling => node.prev_siblings().skip(1).filter(visible).map(wrap).collect(),
            Axis::Attribute => (0..node.attributes().len()).map(|index| Item::Attr(id, index)).collect(),
        }
    }
    fn string(&self, item: Item) -> String {
        match item {
            Item::Attr(id, index) => self.node(id).attributes().nth(index).map(|a| a.value().to_owned()).unwrap_or_default(),
            Item::Node(id) => string_value(self.node(id)),
        }
    }
    fn select(&self, path: &Path, context: Item) -> R<Vec<Item>> {
        let mut current = vec![if path.absolute { Item::Node(self.doc.root().id()) } else { context }];
        for step in &path.steps {
            let mut next = Vec::new();
            for item in &current {
                let candidates: Vec<Item> =
                    self.axis(*item, step.axis).into_iter().filter(|c| self.matches(*c, step.axis, &step.test)).collect();
                let mut kept = candidates;
                for predicate in &step.predicates {
                    let size = kept.len();
                    let mut filtered = Vec::new();
                    for (position, candidate) in kept.iter().enumerate() {
                        let position = position + 1;
                        if self.truth(predicate, *candidate, position, size)? {
                            filtered.push(*candidate);
                        }
                    }
                    kept = filtered;
                }
                next.extend(kept);
                limits::require(next.len() <= 65536, "xml_path_limit")?;
            }
            next.sort_by_key(|item| item.order());
            next.dedup();
            current = next;
        }
        Ok(current)
    }
    fn truth(&self, predicate: &Pred, item: Item, position: usize, size: usize) -> R<bool> {
        Ok(match self.eval(predicate, item, position, size)? {
            Val::Num(n) => n == position as f64,
            other => self.boolean(&other),
        })
    }
    fn boolean(&self, value: &Val) -> bool {
        match value {
            Val::Nodes(nodes) => !nodes.is_empty(),
            Val::Str(text) => !text.is_empty(),
            Val::Num(n) => *n != 0.0 && !n.is_nan(),
            Val::Bool(b) => *b,
        }
    }
    fn text(&self, value: &Val) -> String {
        match value {
            Val::Nodes(nodes) => nodes.first().map(|n| self.string(*n)).unwrap_or_default(),
            Val::Str(text) => text.clone(),
            Val::Num(n) => {
                if n.fract() == 0.0 && n.is_finite() { format!("{}", *n as i64) } else { n.to_string() }
            }
            Val::Bool(b) => b.to_string(),
        }
    }
    fn number(&self, value: &Val) -> f64 {
        match value {
            Val::Num(n) => *n,
            Val::Bool(b) => f64::from(u8::from(*b)),
            other => self.text(other).trim().parse().unwrap_or(f64::NAN),
        }
    }
    fn eval(&self, predicate: &Pred, item: Item, position: usize, size: usize) -> R<Val> {
        Ok(match predicate {
            Pred::Str(text) => Val::Str(text.clone()),
            Pred::Num(n) => Val::Num(*n),
            Pred::Path(path) => Val::Nodes(self.select(path, item)?),
            Pred::Or(a, b) => Val::Bool(self.truth(a, item, position, size)? || self.truth(b, item, position, size)?),
            Pred::And(a, b) => Val::Bool(self.truth(a, item, position, size)? && self.truth(b, item, position, size)?),
            Pred::Cmp(a, op, b) => {
                let (left, right) = (self.eval(a, item, position, size)?, self.eval(b, item, position, size)?);
                let lefts: Vec<Val> = match left {
                    Val::Nodes(nodes) => nodes.iter().map(|n| Val::Str(self.string(*n))).collect(),
                    other => vec![other],
                };
                let rights: Vec<Val> = match right {
                    Val::Nodes(nodes) => nodes.iter().map(|n| Val::Str(self.string(*n))).collect(),
                    other => vec![other],
                };
                let mut any = false;
                for l in &lefts {
                    for r in &rights {
                        let numeric = matches!(l, Val::Num(_)) || matches!(r, Val::Num(_)) || !matches!(*op, "=" | "!=");
                        let result = if numeric {
                            let (x, y) = (self.number(l), self.number(r));
                            match *op {
                                "=" => x == y,
                                "!=" => x != y,
                                "<" => x < y,
                                "<=" => x <= y,
                                ">" => x > y,
                                _ => x >= y,
                            }
                        } else {
                            let equal = self.text(l) == self.text(r);
                            if *op == "=" { equal } else { !equal }
                        };
                        any |= result;
                    }
                }
                Val::Bool(any)
            }
            Pred::Call(name, args) => {
                let arg = |n: usize| -> R<Val> {
                    args.get(n).map(|a| self.eval(a, item, position, size)).unwrap_or(Ok(Val::Nodes(vec![item])))
                };
                let local = |value: Val| -> String {
                    match value {
                        Val::Nodes(nodes) => nodes
                            .first()
                            .map(|n| match n {
                                Item::Attr(id, index) => {
                                    self.node(*id).attributes().nth(*index).map(|a| a.name().to_owned()).unwrap_or_default()
                                }
                                Item::Node(id) => self.node(*id).tag_name().name().to_owned(),
                            })
                            .unwrap_or_default(),
                        _ => String::new(),
                    }
                };
                match name.as_str() {
                    "last" => Val::Num(size as f64),
                    "position" => Val::Num(position as f64),
                    "local-name" | "name" => Val::Str(local(arg(0)?)),
                    "string" => Val::Str(self.text(&arg(0)?)),
                    "normalize-space" => Val::Str(self.text(&arg(0)?).split_whitespace().collect::<Vec<_>>().join(" ")),
                    "string-length" => Val::Num(self.text(&arg(0)?).chars().count() as f64),
                    "contains" => Val::Bool(self.text(&arg(0)?).contains(&self.text(&arg(1)?))),
                    "starts-with" => Val::Bool(self.text(&arg(0)?).starts_with(&self.text(&arg(1)?))),
                    "count" => match arg(0)? {
                        Val::Nodes(nodes) => Val::Num(nodes.len() as f64),
                        _ => return Err(Fail::invalid()),
                    },
                    "not" => Val::Bool(!self.truth(args.first().ok_or_else(Fail::invalid)?, item, position, size)?),
                    "true" => Val::Bool(true),
                    "false" => Val::Bool(false),
                    "number" => Val::Num(self.number(&arg(0)?)),
                    _ => return Err(Fail::invalid()),
                }
            }
        })
    }
}

/// `XPathNavigator.SelectSingleNode(path)?.Value` on a parsed document.
pub(super) fn select_single(document: &Document, path: &str) -> R<Option<String>> {
    let chars: Vec<char> = path.chars().collect();
    let mut parser = P { s: &chars, i: 0 };
    let parsed = parser.path()?;
    parser.ws();
    limits::require(parser.i == chars.len(), "invalid_expression_or_request")?;
    let ctx = Ctx { doc: document };
    let found = ctx.select(&parsed, Item::Node(document.root().id()))?;
    Ok(found.first().map(|item| ctx.string(*item)))
}

/// Elements in document order with their sensitive ancestry for body credential scanning.
pub(super) fn scan_body(document: &Document, add: &mut dyn FnMut(&str, &str) -> R<()>) -> R<()> {
    let sensitive = super::sensitive;
    let mut elements = 0usize;
    for node in document.root().descendants().filter(Node::is_element) {
        elements += 1;
        limits::require(elements <= 65536, "body_nodes")?;
        let mut auth = false;
        let mut cookie: Option<&str> = None;
        for ancestor in node.ancestors().filter(Node::is_element) {
            let name = ancestor.tag_name().name();
            auth |= sensitive(name);
            if cookie.is_none() && (name.eq_ignore_ascii_case("Cookie") || name.eq_ignore_ascii_case("Set-Cookie")) {
                cookie = Some(name);
            }
        }
        if auth {
            match cookie {
                None => add(node.tag_name().name(), &string_value(node))?,
                Some(scope) => {
                    // Individual text values are headers; never concatenate sibling elements.
                    for text in node.children().filter(Node::is_text) {
                        let value = text.text().unwrap_or_default();
                        if !value.trim().is_empty() {
                            add(scope, value)?;
                        }
                    }
                }
            }
        }
        for attribute in node.attributes() {
            let name = attribute.name();
            if auth || sensitive(name) {
                let scope = if name.eq_ignore_ascii_case("Cookie") || name.eq_ignore_ascii_case("Set-Cookie") {
                    name
                } else {
                    cookie.unwrap_or(name)
                };
                add(scope, attribute.value())?;
            }
        }
    }
    Ok(())
}
