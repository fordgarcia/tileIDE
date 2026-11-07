// SPDX-License-Identifier: GPL-3.0-or-later
//! Java projects (v0.6 phase 5): a source's package, and the Maven or Gradle build
//! around it, which compiles it and gives its classpath (dependencies included) to
//! `Alt+R` and `F5`.

use std::path::{Path, PathBuf};

/// The `package` a Java source declares (`package a.b;`), if any.
pub fn package_of(source: &str) -> Option<String> {
    let mut in_comment = false;
    for line in source.lines() {
        let mut line = line.trim();
        if in_comment {
            let Some(end) = line.find("*/") else { continue };
            line = line[end + 2..].trim();
            in_comment = false;
        }
        if line.starts_with("/*") {
            in_comment = !line.contains("*/");
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let rest = line.strip_prefix("package")?.trim_start();
        return rest.split(';').next().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tool {
    Maven,
    Gradle,
}

/// The build a Java file belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    pub tool: Tool,
    /// Where the build runs: the top project (a multi-module build's root).
    pub root: PathBuf,
    /// The module (project) holding the file.
    pub module: PathBuf,
    /// A test source (`src/test/…`): built with its test classes and test dependencies.
    pub test: bool,
}

const GRADLE_FILES: &[&str] = &["build.gradle", "build.gradle.kts"];
const GRADLE_SETTINGS: &[&str] = &["settings.gradle", "settings.gradle.kts"];

/// Registers `tileideClasspath` on the project in `tileide.project`: compiles its
/// main (or test) classes and writes their runtime classpath to `tileide.out`.
const GRADLE_INIT: &str = r#"def target = new File(System.getProperty('tileide.project')).canonicalFile
def out = new File(System.getProperty('tileide.out'))
def test = Boolean.getBoolean('tileide.test')
gradle.allprojects { p ->
    if (p.projectDir.canonicalFile == target) {
        p.plugins.withId('java') {
            p.tasks.register('tileideClasspath') {
                def set = p.sourceSets.getByName(test ? 'test' : 'main')
                dependsOn set.classesTaskName
                def files = set.runtimeClasspath
                doLast { out.text = files.asPath }
            }
        }
    }
}
"#;

fn has_any(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| dir.join(n).is_file())
}

/// The Maven or Gradle build around `file` (not above `stop`): the nearest folder
/// with a `pom.xml` or `build.gradle(.kts)`, and the top of its build.
pub fn build_of(file: &Path, stop: Option<&Path>) -> Option<Build> {
    let within = |d: &Path| stop.is_none_or(|s| d.starts_with(s));
    let module = file.ancestors().skip(1).take_while(|d| within(d)).find(|d| d.join("pom.xml").is_file() || has_any(d, GRADLE_FILES))?;
    let test = file.strip_prefix(module).is_ok_and(|rel| rel.starts_with("src/test"));
    let (tool, root) = if module.join("pom.xml").is_file() {
        // The highest folder in an unbroken run of pom.xml folders.
        let root = module.ancestors().take_while(|d| within(d) && d.join("pom.xml").is_file()).last().unwrap_or(module);
        (Tool::Maven, root)
    } else {
        let root = module.ancestors().take_while(|d| within(d)).find(|d| has_any(d, GRADLE_SETTINGS)).unwrap_or(module);
        (Tool::Gradle, root)
    };
    Some(Build { tool, root: root.to_path_buf(), module: module.to_path_buf(), test })
}

impl Build {
    /// The build's own launcher (`mvnw`, `gradlew`) if it has one, else the tool's.
    pub fn program(&self) -> String {
        let (wrapper, tool) = match self.tool {
            Tool::Maven => ("mvnw", "mvn"),
            Tool::Gradle => ("gradlew", "gradle"),
        };
        let own = self.root.join(wrapper);
        if own.is_file() { own.to_string_lossy().into_owned() } else { tool.to_string() }
    }

    /// The command that compiles the module and writes its classpath (without the
    /// module's own classes for Maven: `classes`) to `classpath_file`. Gradle's init
    /// script is written into `out_dir`.
    pub fn compile(&self, classpath_file: &Path, out_dir: &Path) -> Result<Vec<String>, String> {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let mut cmd = vec![self.program()];
        match self.tool {
            Tool::Maven => {
                cmd.extend(["-q".into(), "-B".into(), "-f".into(), path(&self.root.join("pom.xml"))]);
                let module = self.module.strip_prefix(&self.root).unwrap_or(Path::new(""));
                if !module.as_os_str().is_empty() {
                    // Only this module, with the modules it needs built in the same run.
                    cmd.extend(["-pl".into(), path(module), "-am".into()]);
                }
                let (phase, scope) = if self.test { ("test-compile", "test") } else { ("compile", "runtime") };
                cmd.extend([phase.into(), "dependency:build-classpath".into(), format!("-Dmdep.outputFile={}", path(classpath_file)), format!("-Dmdep.includeScope={scope}")]);
            }
            Tool::Gradle => {
                let script = out_dir.join("tileide-classpath.gradle");
                std::fs::create_dir_all(out_dir).and_then(|()| std::fs::write(&script, GRADLE_INIT)).map_err(|e| format!("{}: {e}", script.display()))?;
                cmd.extend(["-q".into(), "--console=plain".into(), "-p".into(), path(&self.root), "--init-script".into(), path(&script)]);
                cmd.extend([format!("-Dtileide.project={}", path(&self.module)), format!("-Dtileide.out={}", path(classpath_file)), format!("-Dtileide.test={}", self.test), "tileideClasspath".into()]);
            }
        }
        Ok(cmd)
    }

    /// The module's compiled classes, ahead of the dependencies in the classpath
    /// file (Gradle's classpath already holds them).
    pub fn classes(&self) -> Vec<PathBuf> {
        match self.tool {
            Tool::Maven if self.test => vec![self.module.join("target/test-classes"), self.module.join("target/classes")],
            Tool::Maven => vec![self.module.join("target/classes")],
            Tool::Gradle => Vec::new(),
        }
    }

    /// The module's Java source folders that exist (`src/main/java`, `src/test/java`).
    pub fn source_roots(&self) -> Vec<PathBuf> {
        ["src/main/java", "src/test/java"].iter().map(|r| self.module.join(r)).filter(|d| d.is_dir()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_maven_and_gradle_builds() {
        let dir = std::env::temp_dir().join(format!("tileide-java-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mvn = dir.join("m/app/src/test/java/a");
        let gradle = dir.join("g/lib/src/main/java/b");
        std::fs::create_dir_all(&mvn).unwrap();
        std::fs::create_dir_all(&gradle).unwrap();
        for f in ["m/pom.xml", "m/app/pom.xml", "g/settings.gradle.kts", "g/lib/build.gradle.kts"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let b = build_of(&mvn.join("T.java"), Some(&dir)).unwrap();
        assert_eq!((b.tool, b.root.clone(), b.module.clone(), b.test), (Tool::Maven, dir.join("m"), dir.join("m/app"), true));
        let cmd = b.compile(Path::new("/o/cp"), Path::new("/o")).unwrap();
        assert_eq!(cmd[..4], ["mvn", "-q", "-B", "-f"]);
        assert!(cmd.windows(3).any(|w| w == ["-pl", "app", "-am"]));
        assert!(cmd.contains(&"test-compile".to_string()) && cmd.contains(&"-Dmdep.includeScope=test".to_string()));
        assert_eq!(b.classes(), [dir.join("m/app/target/test-classes"), dir.join("m/app/target/classes")]);
        assert_eq!(b.source_roots(), [dir.join("m/app/src/test/java")]);
        let g = build_of(&gradle.join("M.java"), Some(&dir)).unwrap();
        assert_eq!((g.tool, g.root.clone(), g.module.clone(), g.test), (Tool::Gradle, dir.join("g"), dir.join("g/lib"), false));
        std::fs::write(dir.join("g/gradlew"), "").unwrap();
        assert_eq!(g.program(), dir.join("g/gradlew").to_string_lossy());
        // `stop` bounds the search: a file outside any build has none.
        assert_eq!(build_of(&dir.join("x/A.java"), Some(&dir)), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
