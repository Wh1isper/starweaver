/// Escape text for XML text nodes.
#[must_use]
pub fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape text for XML attribute values.
#[must_use]
pub fn escape_xml_attribute(value: &str) -> String {
    escape_xml_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Small XML writer for deterministic model-facing context blocks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct XmlWriter {
    output: String,
    indent: usize,
}

impl XmlWriter {
    /// Create an empty XML writer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            output: String::new(),
            indent: 0,
        }
    }

    /// Open an element with no attributes.
    pub fn open(&mut self, name: &str) -> &mut Self {
        self.open_attrs(name, std::iter::empty::<(&str, &str)>())
    }

    /// Open an element with attributes.
    pub fn open_attrs<K, V, I>(&mut self, name: &str, attrs: I) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.write_indent();
        self.output.push('<');
        self.output.push_str(name);
        self.write_attrs(attrs);
        self.output.push_str(">\n");
        self.indent = self.indent.saturating_add(1);
        self
    }

    /// Close an element.
    pub fn close(&mut self, name: &str) -> &mut Self {
        self.indent = self.indent.saturating_sub(1);
        self.write_indent();
        self.output.push_str("</");
        self.output.push_str(name);
        self.output.push_str(">\n");
        self
    }

    /// Write a text element with no attributes.
    pub fn text_element(&mut self, name: &str, text: impl AsRef<str>) -> &mut Self {
        self.text_element_attrs(name, std::iter::empty::<(&str, &str)>(), text)
    }

    /// Write a text element with attributes.
    pub fn text_element_attrs<K, V, I>(
        &mut self,
        name: &str,
        attrs: I,
        text: impl AsRef<str>,
    ) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.write_indent();
        self.output.push('<');
        self.output.push_str(name);
        self.write_attrs(attrs);
        self.output.push('>');
        self.output.push_str(&escape_xml_text(text.as_ref()));
        self.output.push_str("</");
        self.output.push_str(name);
        self.output.push_str(">\n");
        self
    }

    /// Write a multiline text element with attributes.
    pub fn text_block_element_attrs<K, V, I>(
        &mut self,
        name: &str,
        attrs: I,
        text: impl AsRef<str>,
    ) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.write_indent();
        self.output.push('<');
        self.output.push_str(name);
        self.write_attrs(attrs);
        self.output.push_str(">\n");
        self.output.push_str(&escape_xml_text(text.as_ref()));
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.write_indent();
        self.output.push_str("</");
        self.output.push_str(name);
        self.output.push_str(">\n");
        self
    }

    /// Write an empty element with attributes.
    pub fn empty_element_attrs<K, V, I>(&mut self, name: &str, attrs: I) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
        I: IntoIterator<Item = (K, V)>,
    {
        self.write_indent();
        self.output.push('<');
        self.output.push_str(name);
        self.write_attrs(attrs);
        self.output.push_str(" />\n");
        self
    }

    /// Finish and return the rendered XML string.
    #[must_use]
    pub fn finish(mut self) -> String {
        if self.output.ends_with('\n') {
            self.output.pop();
        }
        self.output
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
    }

    fn write_attrs<K, V, I>(&mut self, attrs: I)
    where
        K: AsRef<str>,
        V: AsRef<str>,
        I: IntoIterator<Item = (K, V)>,
    {
        for (key, value) in attrs {
            self.output.push(' ');
            self.output.push_str(key.as_ref());
            self.output.push_str("=\"");
            self.output.push_str(&escape_xml_attribute(value.as_ref()));
            self.output.push('"');
        }
    }
}
