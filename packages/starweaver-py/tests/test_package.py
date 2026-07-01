import starweaver


def test_version_matches_native_extension() -> None:
    assert starweaver.__version__ == starweaver.version()
