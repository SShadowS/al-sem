from agentflow.sanitize import norm_path, scan, scan_file

CDO = r"U:\Git\DO.Support-SlowDOSetup\DocumentOutput\Cloud"


def test_norm_path_folds_slashes_and_case():
    assert norm_path(r"U:\Git\X\\") == "u:/git/x"


def test_cdo_paths_in_either_slash_style_are_caught():
    text = "ok line\nsee u:/git/do.support-slowdosetup/documentoutput/cloud/App/x.al\nand U:\\Git\\DO.Support-SlowDOSetup\\DocumentOutput\\Cloud\\y.al"
    v = scan(text, CDO)
    assert [x.kind for x in v] == ["cdo-path", "cdo-path"] and [x.line_no for x in v] == [2, 3]


def test_alpackages_and_tokens_caught():
    text = "C:/proj/.alpackages/Microsoft_Base.app\nToken ghp_" + "a" * 36 + "\ngithub_pat_" + "b" * 30 + "\nAuthorization: Bearer " + "c" * 40
    kinds = [x.kind for x in scan(text, None)]
    assert kinds == ["alpackages-path", "token", "token", "token"]


def test_dependencies_folder_is_not_flagged():
    assert scan("Al/.dependencies/Foo/Bar.al is ordinary source", None) == []


def test_clean_text_and_file(tmp_path):
    assert scan("nothing here\n", CDO) == []
    f = tmp_path / "ledger.md"
    f.write_text("fine\n" + CDO + "\n", encoding="utf-8")
    assert [x.line_no for x in scan_file(f, CDO)] == [2]
