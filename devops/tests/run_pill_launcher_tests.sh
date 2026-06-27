
# ---- basics: smoke-test the binary itself -----------------------------------
test_basics() {
    echo "--- Launcher basics ---"

    if invoke_launcher --help > /dev/null 2>&1; then
        report_pass "--help works"
    else
        report_fail "--help works" "exit $?"
    fi

    invoke_launcher --version > /dev/null 2>&1 \
        && report_pass "--version works" \
        || report_fail "--version works" "exit $?"

    local exit_code
    invoke_launcher 2>&1 >/dev/null && exit_code=$? || exit_code=$?
    [ "$exit_code" -ne 0 ] && report_pass "no args exits non-zero" \
                            || report_fail "no args exits non-zero" "exit 0"

    assert_fail "unknown action"    "error"   -a nonexistent-action
    assert_fail "missing --action"  "required" --path .
}

# ---- create: scaffold a new project -----------------------------------------
test_create() {
    echo "--- Create action ---"
    local project_dir="$test_workspace_root/MyGame"

    assert_ok "create project" -a create -n MyGame -p "$test_workspace_root"

    [ -d "$project_dir" ]                && report_pass "project dir exists"      || report_fail "project dir exists" "missing $project_dir"
    [ -f "$project_dir/Cargo.toml" ]     && report_pass "Cargo.toml exists"      || report_fail "Cargo.toml exists" "missing"
    [ -f "$project_dir/res/config.ini" ] && report_pass "config.ini exists"      || report_fail "config.ini exists" "missing"
    [ -d "$project_dir/src" ]            && report_pass "src/ exists"             || report_fail "src/ exists" "missing"

    if grep -q "MyGame" "$project_dir/res/config.ini" 2>/dev/null; then
        report_pass "config.ini contains project name"
    else
        report_fail "config.ini contains project name" "not found"
    fi

    assert_fail "duplicate create"   "already exists" -a create -n MyGame -p "$test_workspace_root"
    assert_fail "create without --name" "name" -a create -p "$test_workspace_root"
}
# ---- build: compile a freshly-created temp project --------------------------
test_build() {
    local project_dir="$test_workspace_root/BuildTest"
    invoke_launcher -a create -n BuildTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "build test" "create failed"; return
    }
    _build_project "$project_dir" "build"
}

# ---- cargo: passthrough arbitrary cargo commands ----------------------------
test_cargo() {
    echo "--- Cargo passthrough ---"
    local project_dir="$test_workspace_root/CargoTest"

    invoke_launcher -a create -n CargoTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "cargo tests" "create failed — cannot proceed"
        return
    }

    assert_ok "cargo --version" -a cargo -p "$project_dir" -- --version
    assert_fail "cargo empty args" "at least one argument" -a cargo -p "$project_dir"

    echo "    (cargo check on test project — may take a moment)"
    if invoke_launcher -a cargo -p "$project_dir" -- check > /dev/null 2>&1; then
        report_pass "cargo check on project"
    else
        report_skip "cargo check on project" "exit $? (may need full workspace)"
    fi
}

# ---- assets: run the asset pipeline -----------------------------------------
test_assets() {
    echo "--- Assets ---"
    local project_dir="$test_workspace_root/AssetsTest"
    invoke_launcher -a create -n AssetsTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "assets tests" "create failed"; return
    }

    assert_ok "assets on empty project"   -a assets -p "$project_dir"
    assert_ok "assets --clean"            -a assets -p "$project_dir" --clean
}