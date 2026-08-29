pub mod autotools_cmd;
pub mod bazel_cmd;
pub mod buck2_cmd;
pub mod build2_cmd;
pub mod cmake_cmd;
pub mod diag;
pub mod make_cmd;
pub mod meson_cmd;
pub mod msbuild_cmd;
pub mod ninja_cmd;
pub mod nmake_cmd;
pub mod premake_cmd;
pub mod scons_cmd;
pub mod ubt_cmd;
pub mod xmake_cmd;

/// Identifies which C++ build tool a command belongs to.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Cmake,
    Ninja,
    Make,
    Autotools,
    Meson,
    MSBuild,
    NMake,
    Bazel,
    Buck2,
    SCons,
    Xmake,
    Premake,
    Build2,
    Ubt,
}

#[allow(dead_code)]
impl ToolKind {
    /// Detect from command name (unambiguous cases).
    pub fn from_command(cmd: &str) -> Option<Self> {
        match cmd.to_lowercase().as_str() {
            "cmake" => Some(Self::Cmake),
            "ninja" => Some(Self::Ninja),
            "make" | "gmake" => Some(Self::Make),
            "autoconf" | "configure" => Some(Self::Autotools),
            "meson" => Some(Self::Meson),
            "msbuild" => Some(Self::MSBuild),
            "nmake" => Some(Self::NMake),
            "bazel" | "bazelisk" => Some(Self::Bazel),
            "buck2" => Some(Self::Buck2),
            "scons" => Some(Self::SCons),
            "xmake" => Some(Self::Xmake),
            "premake5" | "premake" => Some(Self::Premake),
            "b" | "bpkg" => Some(Self::Build2),
            "ubt" | "unrealbuildtool" => Some(Self::Ubt),
            _ => None,
        }
    }

    /// Detect from first line of output (grammar sniffing).
    pub fn from_output_first_line(line: &str) -> Option<Self> {
        let stripped = crate::core::utils::strip_ansi(line);
        let trimmed = stripped.trim();

        // CMake configure: "-- The C compiler identification is ..."
        if trimmed.starts_with("-- The ") && trimmed.contains("compiler identification") {
            return Some(Self::Cmake);
        }

        // Ninja progress: "[N/M] Building ..."
        if ninja_cmd::is_progress_line(trimmed) {
            return Some(Self::Ninja);
        }

        // Autotools: "checking build system type..."
        if trimmed.starts_with("checking build system type") {
            return Some(Self::Autotools);
        }

        // Meson: "The Meson build system"
        if trimmed.starts_with("The Meson build system") {
            return Some(Self::Meson);
        }

        // MSBuild: "Microsoft (R) Build Engine"
        if trimmed.contains("Microsoft (R) Build Engine") {
            return Some(Self::MSBuild);
        }

        // Bazel: "Loading:" or "Analyzing:"
        if trimmed.starts_with("Loading:") || trimmed.starts_with("Analyzing:") {
            return Some(Self::Bazel);
        }

        // Buck2: "Build ID:"
        if trimmed.starts_with("Build ID:") {
            return Some(Self::Buck2);
        }

        // SCons: "scons: Reading SConscript"
        if trimmed.starts_with("scons: Reading SConscript") {
            return Some(Self::SCons);
        }

        // Xmake: "=== XMAKE "
        if trimmed.starts_with("=== XMAKE ") {
            return Some(Self::Xmake);
        }

        // Premake: "Building configurations..."
        if trimmed.starts_with("Building configurations") {
            return Some(Self::Premake);
        }

        // build2: action lines like "c++ foo.cxx@/proj/" or "ld /proj/app/"
        if build2_cmd::is_action_line(trimmed) {
            return Some(Self::Build2);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolkind_from_command_known() {
        assert_eq!(ToolKind::from_command("cmake"), Some(ToolKind::Cmake));
        assert_eq!(ToolKind::from_command("ninja"), Some(ToolKind::Ninja));
        assert_eq!(ToolKind::from_command("make"), Some(ToolKind::Make));
        assert_eq!(ToolKind::from_command("gmake"), Some(ToolKind::Make));
        assert_eq!(
            ToolKind::from_command("configure"),
            Some(ToolKind::Autotools)
        );
        assert_eq!(ToolKind::from_command("meson"), Some(ToolKind::Meson));
        assert_eq!(ToolKind::from_command("msbuild"), Some(ToolKind::MSBuild));
        assert_eq!(ToolKind::from_command("nmake"), Some(ToolKind::NMake));
        assert_eq!(ToolKind::from_command("bazel"), Some(ToolKind::Bazel));
        assert_eq!(ToolKind::from_command("bazelisk"), Some(ToolKind::Bazel));
        assert_eq!(ToolKind::from_command("buck2"), Some(ToolKind::Buck2));
        assert_eq!(ToolKind::from_command("scons"), Some(ToolKind::SCons));
        assert_eq!(ToolKind::from_command("xmake"), Some(ToolKind::Xmake));
        assert_eq!(ToolKind::from_command("premake5"), Some(ToolKind::Premake));
        assert_eq!(ToolKind::from_command("premake"), Some(ToolKind::Premake));
        assert_eq!(ToolKind::from_command("b"), Some(ToolKind::Build2));
        assert_eq!(ToolKind::from_command("bpkg"), Some(ToolKind::Build2));
        assert_eq!(ToolKind::from_command("ubt"), Some(ToolKind::Ubt));
        assert_eq!(
            ToolKind::from_command("UnrealBuildTool"),
            Some(ToolKind::Ubt)
        );
    }

    #[test]
    fn test_toolkind_from_command_unknown() {
        assert_eq!(ToolKind::from_command("unknown_tool_xyz"), None);
        assert_eq!(ToolKind::from_command(""), None);
    }

    #[test]
    fn test_toolkind_from_output_cmake() {
        assert_eq!(
            ToolKind::from_output_first_line("-- The C compiler identification is GNU 13.2.0"),
            Some(ToolKind::Cmake)
        );
    }

    #[test]
    fn test_toolkind_from_output_ninja() {
        assert_eq!(
            ToolKind::from_output_first_line("[1/456] Building CXX object src/core/clock.cpp.o"),
            Some(ToolKind::Ninja)
        );
    }

    #[test]
    fn test_toolkind_from_output_meson() {
        assert_eq!(
            ToolKind::from_output_first_line("The Meson build system"),
            Some(ToolKind::Meson)
        );
    }

    #[test]
    fn test_toolkind_from_output_unknown() {
        assert_eq!(ToolKind::from_output_first_line("some random output"), None);
    }
}
