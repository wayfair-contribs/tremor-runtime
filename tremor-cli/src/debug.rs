// Copyright 2020, The Tremor Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::errors::Result;
use crate::util::{get_source_kind, SourceKind};
use clap::ArgMatches;
use io::BufReader;
use lexer::Tokenizer;
use std::io::Write;
use std::io::{self, Read};
use termcolor::{Color, ColorSpec};
use tremor_script::highlighter::{Dumb as TermNoHighlighter, Highlighter, Term as TermHighlighter};
use tremor_script::lexer;
use tremor_script::lexer::Token;
use tremor_script::path::load as load_module_path;
use tremor_script::pos::{Span, Spanned};
use tremor_script::query::Query;
use tremor_script::registry;
use tremor_script::registry::Registry;
use tremor_script::script::Script;

struct Opts<'src> {
    banner: bool,
    kind: SourceKind,
    src: &'src str,
    raw: String,
}

fn banner<W>(h: &mut W, opts: &Opts, section: &str, detail: &str) -> Result<()>
where
    W: Highlighter,
{
    if opts.banner {
        let mut banner = ColorSpec::new();
        let mut banner = banner.set_fg(Some(Color::Green));

        h.set_color(&mut banner)?;
        let spec = format!(
            "\n\n****************\n* {} - {}\n****************\n\n",
            section, detail
        );
        write!(h.get_writer(), "{}", spec,)?;
    }
    Ok(())
}

fn dbg_src<W>(h: &mut W, opts: &Opts) -> Result<()>
where
    W: Highlighter,
{
    banner(h, opts, "Source", "Source code listing")?;
    match opts.kind {
        SourceKind::Tremor | SourceKind::Json | SourceKind::Default => {
            Script::highlight_script_with(&opts.raw, h)?
        }
        SourceKind::Trickle => Query::highlight_script_with(&opts.raw, h)?,
        SourceKind::Unsupported => {
            eprintln!("Unsupported");
        }
    }

    Ok(())
}

fn dbg_tokens<W>(h: &mut W, lexemes: Vec<Spanned<Token>>) -> Result<()>
where
    W: Highlighter,
{
    let mut default = ColorSpec::new();
    let mut line = ColorSpec::new();
    let mut line = line.set_fg(Some(Color::Blue));
    let mut directive = ColorSpec::new();
    let mut directive = directive.set_fg(Some(Color::White));

    for l in lexemes {
        let Spanned {
            span:
                Span {
                    start,
                    end,
                    pp_start,
                    pp_end,
                },
            value,
        } = l;
        match &value {
                // We ignore whitespace and newlines
                Token::Whitespace(_)
                | Token::NewLine
                // TODO Enhance to support doc/mod ( multi-line ) comments
                | Token::DocComment(_)
                | Token::ModComment(_) => (),
                Token::ConfigDirective => {
                    h.set_color(&mut line)?;
                    let line_spec = format!(
                        "{}:{} - {}:{}",
                        start.line(), start.column(), end.line(), end.column()
                    );
                    write!(h.get_writer(), "{:^16} \u{2219}    ", line_spec,)?;
                    h.set_color(&mut directive)?;
                    writeln!(h.get_writer(), " #!config ")?;
                }
                Token::LineDirective(_location, file) => {
                    h.set_color(&mut line)?;
                    let line_spec = format!(
                        "{}:{} - {}:{}",
                        start.line(), start.column(), end.line(), end.column()
                    );
                    write!(h.get_writer(), "{:^16} \u{2219}    ", line_spec,)?;
                    h.set_color(&mut directive)?;
                    writeln!(h.get_writer(), " #!line {}", file.to_string())?;
                }
                _other_token => {
                    h.set_color(&mut line)?;
                    let line_spec = format!(
                        "{}:{} - {}:{}",
                        start.line(), start.column(), end.line(), end.column()
                    );
                    write!(h.get_writer(), "{:^16} \u{2219}    ", line_spec,)?;
                    h.set_color(&mut default)?;
                    write!(
                        &mut h.get_writer(),
                        " {:<30}    \u{2219}    ",
                        format!("{:?}", value).split('(').collect::<Vec<&str>>()[0]
                    )?;
                    h.highlight_no_linenos(
                        None,
                        &[Spanned {
                            span: Span { start, end, pp_start, pp_end },
                            value,
                        }],
                    )?;
                }
            }
    }

    Ok(())
}

fn dbg_pp<W>(h: &mut W, opts: &Opts) -> Result<()>
where
    W: Highlighter,
{
    banner(
        h,
        opts,
        "Lexemes",
        "Lexical token stream after preprocessing",
    )?;

    let lexemes: Vec<_> = Tokenizer::new(&opts.raw)
        .filter_map(std::result::Result::ok)
        .collect();

    dbg_tokens(h, lexemes)?;

    h.reset()?;

    Ok(())
}

fn dbg_lex<W>(h: &mut W, opts: &Opts) -> Result<()>
where
    W: Highlighter,
{
    banner(
        h,
        opts,
        "Lexemes",
        "Lexical token stream before preprocessing",
    )?;

    let mut include_stack = lexer::IncludeStack::default();
    let cu = include_stack.push(&opts.src)?;
    let mut src_raw_string = opts.raw.clone();

    let lexemes: Vec<_> = lexer::Preprocessor::preprocess(
        &tremor_script::path::load(),
        opts.src,
        &mut src_raw_string,
        cu,
        &mut include_stack,
    )?
    .into_iter()
    .filter_map(std::result::Result::ok)
    .collect();

    dbg_tokens(h, lexemes)?;

    h.reset()?;

    Ok(())
}

fn dbg_ast<'src, W>(h: &mut W, opts: &Opts<'src>) -> Result<()>
where
    W: Highlighter,
{
    banner(h, &opts, "AST", "Abstract Syntax Tree")?;

    let mp = load_module_path();
    let reg: Registry = registry::registry();
    match opts.kind {
        SourceKind::Tremor | SourceKind::Json | SourceKind::Default => {
            match Script::parse(&mp, opts.src, opts.raw.clone(), &reg) {
                Ok(runnable) => {
                    let ast = simd_json::to_string_pretty(&runnable.script.suffix())?;
                    println!();
                    Script::highlight_script_with(&ast, h)?;
                }
                Err(e) => {
                    if let Err(e) = Script::format_error_from_script(&opts.raw, h, &e) {
                        eprintln!("Error: {}", e);
                    };
                }
            }
        }
        SourceKind::Trickle => {
            let aggr_reg = registry::aggr();
            match Query::parse(&mp, opts.src, &opts.raw, vec![], &reg, &aggr_reg) {
                Ok(runnable) => {
                    let ast = simd_json::to_string_pretty(&runnable.query.suffix())?;
                    println!();
                    Script::highlight_script_with(&ast, h)?;
                }
                Err(e) => {
                    if let Err(e) = Script::format_error_from_script(&opts.raw, h, &e) {
                        eprintln!("Error: {}", e);
                    };
                }
            };
        }
        SourceKind::Unsupported => {
            eprintln!("Unsupported");
        }
    };

    h.reset()?;

    Ok(())
}

fn script_opts(matches: &ArgMatches, no_banner: bool) -> Result<Opts> {
    let src = matches.value_of("SCRIPT");
    let mut raw = String::new();

    let mut buffer: Box<dyn Read> = match src {
        None | Some("-") => Box::new(BufReader::new(io::stdin())),
        Some(data) => Box::new(BufReader::new(crate::open_file(data, None)?)),
    };
    let kind = match src {
        None | Some("-") => SourceKind::Tremor,
        Some(data) => get_source_kind(data),
    };
    let src = match src {
        None | Some("-") => "<stdin>",
        Some(data) => data,
    };
    buffer.read_to_string(&mut raw)?;

    println!();
    raw.push('\n'); // Ensure last token is whitespace
    let opts = Opts {
        banner: !no_banner,
        src,
        kind,
        raw,
    };

    Ok(opts)
}

fn dbg_dot<W>(h: &mut W, opts: &Opts) -> Result<()>
where
    W: Highlighter,
{
    if opts.kind != SourceKind::Trickle && opts.kind != SourceKind::Default {
        return Err("Dot visualisation is only supported for trickle files.".into());
    }
    let mp = load_module_path();
    let reg: Registry = registry::registry();
    let aggr_reg = registry::aggr();
    match Query::parse(&mp, opts.src, &opts.raw, vec![], &reg, &aggr_reg) {
        Ok(runnable) => {
            let mut uid = 0;
            let g = tremor_pipeline::query::Query(runnable).to_pipe(&mut uid)?;

            println!("{}", g.dot)
        }
        Err(e) => {
            if let Err(e) = Script::format_error_from_script(&opts.raw, h, &e) {
                eprintln!("Error: {}", e);
            };
        }
    };
    Ok(())
}

pub(crate) fn run_cmd(matches: &ArgMatches) -> Result<()> {
    let no_highlight = matches.is_present("no-highlight");
    let no_banner = matches.is_present("no-banner");

    if no_highlight {
        let mut h = TermNoHighlighter::new();
        let r = if let Some(args) = matches.subcommand_matches("ast") {
            let opts = script_opts(args, no_banner)?;
            dbg_ast(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("preprocess") {
            let opts = script_opts(args, no_banner)?;
            dbg_pp(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("lex") {
            let opts = script_opts(args, no_banner)?;
            dbg_lex(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("src") {
            let opts = script_opts(args, no_banner)?;
            dbg_src(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("dot") {
            let opts = script_opts(args, no_banner)?;
            dbg_dot(&mut h, &opts)
        } else {
            Err("Missing subcommand".into())
        };
        h.finalize()?;
        h.reset()?;
        println!("{}", h.to_string());
        r?
    } else {
        let mut h = TermHighlighter::default();
        let r = if let Some(args) = matches.subcommand_matches("ast") {
            let opts = script_opts(args, no_banner)?;
            dbg_ast(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("preprocess") {
            let opts = script_opts(args, no_banner)?;
            dbg_pp(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("lex") {
            let opts = script_opts(args, no_banner)?;
            dbg_lex(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("src") {
            let opts = script_opts(args, no_banner)?;
            dbg_src(&mut h, &opts)
        } else if let Some(args) = matches.subcommand_matches("dot") {
            let opts = script_opts(args, no_banner)?;
            dbg_dot(&mut h, &opts)
        } else {
            Err("Missing subcommand".into())
        };
        h.finalize()?;
        h.reset()?;
        r?
    };

    Ok(())
}
