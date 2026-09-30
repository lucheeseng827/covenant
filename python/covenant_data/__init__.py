"""Data contracts, enforced where the data flows.

Check a frame, records or a file against a contract — a ``covenant: 1``
document or an ODCS v3 one — with the engine ``covenant check`` runs::

    import covenant_data

    report = covenant_data.check(df, "orders.yaml")   # polars, pyarrow, DuckDB: zero-copy
    assert report.passed, report

Load a contract once to check many batches::

    contract = covenant_data.Contract("orders.yaml")
    for frame in frames:
        report = contract.check(frame)

A failing check is a ``Report`` whose ``passed`` is false, never an exception;
``CovenantError`` means the contract or the data could not be read at all.
"""

import signal
import sys

from ._native import Contract, CovenantError, Report, __version__, cli

__all__ = ["Contract", "CovenantError", "Report", "check", "main", "__version__"]


def check(data, contract, *, model=None, allow_unenforced=False, source=None):
    """Check ``data`` against ``contract`` and return a :class:`Report`.

    ``data`` is anything that exports Arrow through the PyCapsule interface (a
    polars or pyarrow frame or batch, a DuckDB relation), a list of dicts, or
    the path of an NDJSON, CSV or Parquet file. ``contract`` is a
    :class:`Contract` or the path of a contract file; ``model`` picks one of
    several models, and ``allow_unenforced`` lets an ODCS contract with rules
    Covenant cannot enforce check the rest (the report then says it is
    partial). ``source`` names in-memory data in the report.
    """
    if not isinstance(contract, Contract):
        contract = Contract(contract, model=model, allow_unenforced=allow_unenforced)
    elif model is not None or allow_unenforced:
        raise TypeError("model and allow_unenforced apply when loading a contract from a path")
    return contract.check(data, source=source)


def main():
    """The ``covenant`` command, installed with the wheel."""
    # Ctrl-C stops the command as it stops the binary, rather than waiting to
    # surface as a KeyboardInterrupt after the engine returns.
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    sys.exit(cli(["covenant", *sys.argv[1:]]))
