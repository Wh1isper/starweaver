"""Python bindings for Starweaver."""

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _distribution_version

from ._native import version

try:
    __version__ = _distribution_version("starweaver")
except PackageNotFoundError:
    __version__ = version()

__all__ = ["__version__", "version"]
