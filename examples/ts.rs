use ad_editor::{
    buffer::Buffer,
    dot::Dot,
    term::{Color, Style},
    ts::Parser,
};
use std::{collections::HashMap, error::Error, fs};

const QUERY: &str = "\
(macro_invocation
  macro: (identifier) @function.macro
  \"!\" @function.macro)
(line_comment) @comment
(block_comment) @comment
(char_literal) @string
(string_literal) @string
(raw_string_literal) @string";

fn main() -> Result<(), Box<dyn Error>> {
    let mut parser = Parser::try_new(
        "/home/sminez/.local/share/nvim/lazy/nvim-treesitter/parser",
        "rust",
    )?;
    let content = fs::read_to_string(file!()).unwrap();
    let b = Buffer::new_unnamed(0, content);
    let tree = parser.parse(b.contents(), None).unwrap();
    let root = tree.root_node();

    let mut tkz = parser.new_tokenizer(QUERY).unwrap();
    tkz.update(0, usize::MAX, root, &b);

    // hacked up versions of the in-crate code for this example
    let bg: Color = "#1B1720".try_into().unwrap();
    let fg: Color = "#E6D29E".try_into().unwrap();
    let comment: Color = "#624354".try_into().unwrap();
    let string: Color = "#61DCA5".try_into().unwrap();
    let dot_bg: Color = "#336677".try_into().unwrap();
    let load_bg: Color = "#957FB8".try_into().unwrap();
    let exec_bg: Color = "#Bf616A".try_into().unwrap();

    let cs: HashMap<String, Vec<Style>> = [
        ("default", vec![Style::Bg(bg), Style::Fg(fg)]),
        ("comment", vec![Style::Italic, Style::Fg(comment)]),
        ("string", vec![Style::Fg(string)]),
        ("dot", vec![Style::Fg(fg), Style::Bg(dot_bg)]),
        ("load", vec![Style::Fg(fg), Style::Bg(load_bg)]),
        ("exec", vec![Style::Fg(fg), Style::Bg(exec_bg)]),
        ("function.macro", vec![Style::Fg(load_bg)]),
    ]
    .map(|(s, v)| (s.to_string(), v))
    .into_iter()
    .collect();

    let exec_rng = Some((false, Dot::from_char_indices(56, 90).as_range()));

    // for (i, it) in tkz.iter_tokenized_lines_from(0, &b).enumerate() {
    for it in tkz.iter_tokenized_lines_from(0, &b, exec_rng) {
        let mut buf = String::new();
        // buf.push_str(&format!("{}{i:<2}| ", Style::Fg(exec_bg)));
        for tk in it {
            tk.render(&mut buf, &b, &cs);
            // buf.push_str(&format!("{}|", Style::Fg(exec_bg)));
        }
        println!("{buf}");
    }

    Ok(())
}
