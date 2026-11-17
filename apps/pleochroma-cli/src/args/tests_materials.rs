use super::{
    Command, MarkdownTable, MaterialColumn, MaterialFilter, MaterialScale, MaterialsArgs,
    MaterialsFormat, Topic, parse,
};
use std::path::PathBuf;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn parsed(line: &str) -> MaterialsArgs {
    match parse(&argv(line)) {
        Ok(Command::Materials(args)) => args,
        other => panic!("{line:?} should parse as materials, got {other:?}"),
    }
}

fn error(line: &str) -> String {
    parse(&argv(line)).expect_err("this line should not parse")
}

#[test]
fn materials_alone_is_the_whole_table_in_text() {
    let args = parsed("materials");
    assert_eq!(args.name, None);
    assert_eq!(args.format, MaterialsFormat::Text);
    assert_eq!(args.sort, None);
    assert!(!args.descending);
    assert_eq!(args.filter, MaterialFilter::All);
    assert_eq!(args.scale, MaterialScale::Bg);
    assert!(!args.bands);
    assert_eq!(args.curves, None);
    assert_eq!(args.out, None);
    assert_eq!(args.db, None);
}

#[test]
fn the_table_options_are_read() {
    let args = parsed(
        "materials --sort hardness --desc --filter glass --dispersion fc --out t.txt --db lib.sqlite",
    );
    assert_eq!(args.sort, Some(MaterialColumn::Hardness));
    assert!(args.descending);
    assert_eq!(args.filter, MaterialFilter::Glass);
    assert_eq!(args.scale, MaterialScale::Fc);
    assert_eq!(args.out, Some(PathBuf::from("t.txt")));
    assert_eq!(args.db, Some(PathBuf::from("lib.sqlite")));
}

#[test]
fn every_sort_and_filter_word_is_known() {
    for (word, column) in [
        ("name", MaterialColumn::Name),
        ("system", MaterialColumn::CrystalSystem),
        ("optic", MaterialColumn::OpticCharacter),
        ("ri", MaterialColumn::Ri),
        ("birefringence", MaterialColumn::Birefringence),
        ("dispersion", MaterialColumn::Dispersion),
        ("sg", MaterialColumn::SpecificGravity),
        ("hardness", MaterialColumn::Hardness),
        ("toughness", MaterialColumn::Toughness),
        ("cleavage", MaterialColumn::Cleavage),
    ] {
        assert_eq!(
            parsed(&format!("materials --sort {word}")).sort,
            Some(column)
        );
    }
    for (word, filter) in [
        ("all", MaterialFilter::All),
        ("natural", MaterialFilter::Natural),
        ("synthetic", MaterialFilter::Synthetic),
        ("glass", MaterialFilter::Glass),
        ("custom", MaterialFilter::Custom),
    ] {
        assert_eq!(parsed(&format!("materials --filter {word}")).filter, filter);
    }
    assert!(error("materials --sort colour").contains("unknown column"));
    assert!(error("materials --filter rare").contains("unknown filter"));
    assert!(error("materials --dispersion gg").contains("unknown dispersion"));
}

#[test]
fn a_name_may_be_several_words_and_takes_the_detail_options() {
    let args = parsed("materials Cubic Zirconia --bands --curves n.csv --json");
    assert_eq!(args.name.as_deref(), Some("Cubic Zirconia"));
    assert!(args.bands);
    assert_eq!(args.curves, Some(PathBuf::from("n.csv")));
    assert_eq!(args.format, MaterialsFormat::Json);
}

#[test]
fn the_markdown_tables_are_named() {
    for (word, table) in [
        ("optics", MarkdownTable::Optics),
        ("gem-data", MarkdownTable::GemData),
        ("built-in", MarkdownTable::BuiltIn),
        ("sources", MarkdownTable::Sources),
    ] {
        assert_eq!(
            parsed(&format!("materials --markdown {word}")).format,
            MaterialsFormat::Markdown(table)
        );
    }
    assert!(error("materials --markdown prices").contains("unknown table"));
    assert!(error("materials --markdown").contains("needs a value"));
}

#[test]
fn contradictory_options_are_refused() {
    assert!(error("materials --json --csv").contains("give one of"));
    assert!(error("materials --json --markdown optics").contains("give one of"));
    assert!(error("materials Topaz --csv").contains("leave out the name"));
    assert!(error("materials Topaz --markdown optics").contains("leave out the name"));
    assert!(error("materials Topaz --sort sg").contains("leave out the name"));
    assert!(error("materials Topaz --filter glass").contains("leave out the name"));
    assert!(error("materials --bands").contains("give its name"));
    assert!(error("materials --curves n.csv").contains("give its name"));
    assert_eq!(
        error("materials --frob"),
        "unknown flag \"--frob\" for \"materials\" (see --help)"
    );
}

#[test]
fn the_materials_help_page_is_found() {
    assert_eq!(
        parse(&argv("materials --help")),
        Ok(Command::Help(Topic::Materials))
    );
    assert_eq!(
        parse(&argv("help materials")),
        Ok(Command::Help(Topic::Materials))
    );
    assert!(crate::help::text(Topic::Materials).starts_with("Usage: pleochroma-cli materials "));
}

#[test]
fn materials_is_not_a_design_command() {
    let command = parse(&argv("materials")).expect("parses");
    assert_eq!(command.design(), None);
}
