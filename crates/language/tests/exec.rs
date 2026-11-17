//! Executes plans for real, with a minimal runner written for the test, to
//! prove that the declarative specs are enough to build and run programs.
//! Every case skips visibly when its toolchain is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use scratch_language::{
    CompilePlan, LanguageRegistry, PlannedStep, ResolvedCommand, RunContext, RunPlan, Vars,
};

fn find_in_path(program: &str, path: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return Path::new(program).is_file().then(|| PathBuf::from(program));
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn path_for(plan: &RunPlan) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = plan.path_prepend.clone();
    parts.push(current);
    parts.extend(plan.path_append.clone());
    parts.join(":")
}

/// Runs one step; `Err` carries the message a runner would show.
fn run_step(plan: &RunPlan, step: &PlannedStep, path: &str) -> Result<(), String> {
    for dir in &step.mkdirs {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let use_via = step
        .via
        .first()
        .is_some_and(|w| find_in_path(w, path).is_some());
    let argv = step.argv(use_via);
    let program = find_in_path(&argv[0], path).ok_or_else(|| {
        step.missing_tool
            .clone()
            .unwrap_or_else(|| format!("{} not found", argv[0]))
    })?;
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..])
        .current_dir(&step.cwd)
        .env("PATH", path);
    cmd.envs(plan.process_env()).envs(&step.env);
    let output = cmd.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}{}",
            step.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if let Some(p) = &step.produces
        && p.must_exist
        && !Path::new(&p.value).exists()
    {
        return Err(p
            .missing_message
            .clone()
            .unwrap_or_else(|| format!("{} missing", p.value)));
    }
    Ok(())
}

enum Outcome {
    Skipped(String),
    Output(String),
}

fn execute(id: &str, file: &str, source: &str, extra: &[(&str, &str)]) -> Outcome {
    let registry = LanguageRegistry::builtin();
    let def = registry.get(id).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("src");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(file), source).unwrap();
    for (name, text) in extra {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let out = tmp.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let vars = Vars::for_source(&dir, file).with_tmpdir(&out);
    let plan = def.plan_run(&RunContext::new(vars)).unwrap();
    let path = path_for(&plan);
    if let CompilePlan::Steps(steps) = &plan.compile {
        for step in steps {
            if let Err(message) = run_step(&plan, step, &path) {
                if find_in_path(&step.program, &path).is_none() {
                    return Outcome::Skipped(format!("{id}: {message}"));
                }
                panic!("{id}: {message}");
            }
        }
    }
    let argv = match plan.resolve_command(None).unwrap() {
        ResolvedCommand::Argv(argv) => argv,
        shell @ ResolvedCommand::Shell(_) => shell.to_argv("/bin/bash", false, &[]),
    };
    let Some(program) = find_in_path(&argv[0], &path) else {
        return Outcome::Skipped(format!("{id}: {} not found", argv[0]));
    };
    let output = Command::new(program)
        .args(&argv[1..])
        .current_dir(&dir)
        .env("PATH", &path)
        .envs(plan.process_env())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{id}: run failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Outcome::Output(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn check(id: &str, file: &str, source: &str, expected: &str) {
    check_with(id, file, source, expected, &[]);
}

fn check_with(id: &str, file: &str, source: &str, expected: &str, extra: &[(&str, &str)]) {
    match execute(id, file, source, extra) {
        Outcome::Skipped(why) => eprintln!("SKIPPED: {why}"),
        Outcome::Output(out) => assert_eq!(out, expected, "{id}"),
    }
}

#[test]
fn interpreted_languages_run() {
    check("python", "a.py", "print('hi from py')", "hi from py");
    check("shell", "a.sh", "echo hi from sh", "hi from sh");
    check(
        "javascript-node",
        "a.js",
        "console.log('hi from node')",
        "hi from node",
    );
    check("ruby", "a.rb", "puts 'hi from ruby'", "hi from ruby");
    check("perl", "a.pl", "print \"hi from perl\\n\";", "hi from perl");
    check("lua", "a.lua", "print('hi from lua')", "hi from lua");
    check(
        "php",
        "a.php",
        "<?php echo \"hi from php\\n\";",
        "hi from php",
    );
    check(
        "applescript",
        "a.applescript",
        "return \"hi from osa\"",
        "hi from osa",
    );
}

#[test]
fn compiled_languages_build_and_run() {
    check(
        "c",
        "main.c",
        "#include <stdio.h>\nint main(void){puts(\"hi from c\");return 0;}",
        "hi from c",
    );
    check(
        "cpp",
        "main.cpp",
        "#include <iostream>\nint main(){std::cout<<\"hi from cpp\\n\";}",
        "hi from cpp",
    );
    check(
        "objective-c",
        "main.m",
        "#import <Foundation/Foundation.h>\nint main(void){@autoreleasepool{NSLog(@\"x\");printf(\"hi from objc\\n\");}return 0;}",
        "hi from objc",
    );
    check(
        "swift",
        "main.swift",
        "print(\"hi from swift\")",
        "hi from swift",
    );
    check(
        "rust",
        "main.rs",
        "fn main(){ println!(\"hi from rust\"); }",
        "hi from rust",
    );
    check(
        "go",
        "main.go",
        "package main\nimport \"fmt\"\nfunc main(){ fmt.Println(\"hi from go\") }",
        "hi from go",
    );
    check(
        "typescript-node",
        "a.ts",
        "const x: number = 40 + 2; console.log('hi from ts ' + x);",
        "hi from ts 42",
    );
}

#[test]
fn java_builds_with_a_helper_class_and_runs_from_a_package() {
    check_with(
        "java",
        "Main.java",
        "public class Main { public static void main(String[] a){ System.out.println(Helper.msg()); } }",
        "hi from java",
        &[(
            "Helper.java",
            "class Helper { static String msg(){ return \"hi from java\"; } }",
        )],
    );
}

#[test]
fn c_links_the_implementation_of_included_headers() {
    check_with(
        "c",
        "main.c",
        "#include <stdio.h>\n#include \"util.h\"\nint main(void){printf(\"%d\\n\", twice(21));return 0;}",
        "42",
        &[
            ("util.h", "int twice(int x);\n"),
            (
                "util.c",
                "#include \"util.h\"\nint twice(int x){return x*2;}\n",
            ),
        ],
    );
}

#[test]
fn compile_errors_fail_the_step_with_compiler_output() {
    let registry = LanguageRegistry::builtin();
    let def = registry.get("c").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("bad.c"), "int main(void){ return x; }").unwrap();
    let vars = Vars::for_source(tmp.path(), "bad.c").with_tmpdir(tmp.path());
    let plan = def.plan_run(&RunContext::new(vars)).unwrap();
    let path = path_for(&plan);
    let CompilePlan::Steps(steps) = &plan.compile else {
        panic!()
    };
    if find_in_path("clang", &path).is_none() {
        eprintln!("SKIPPED: clang not found");
        return;
    }
    let message = run_step(&plan, &steps[0], &path).unwrap_err();
    // The output is in the form the diagnostics regex expects.
    let d = def.diagnostics.iter().find(|d| d.id == "clang").unwrap();
    let regex = regex::Regex::new(d.regex.as_deref().unwrap()).unwrap();
    let hit = message.lines().find_map(|l| regex.captures(l));
    let caps = hit.unwrap_or_else(|| panic!("no diagnostic in: {message}"));
    assert_eq!(&caps["line"], "1");
}

#[test]
fn web_languages_produce_their_html() {
    check("html", "a.html", "<h1>hi</h1>", "<h1>hi</h1>");
    check(
        "css",
        "a.css",
        "a{color:red}",
        "<style>\na{color:red}</style>",
    );
    match execute("javascript-browser", "a.js", "1", &[]) {
        Outcome::Output(out) => {
            assert!(out.starts_with("<script src=\"file://"), "{out}");
            assert!(out.ends_with("/src/a.js\"></script>"), "{out}");
        }
        Outcome::Skipped(why) => eprintln!("SKIPPED: {why}"),
    }
}
