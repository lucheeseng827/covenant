"""A test assertion for pytest (or unittest) suites that produce data::

    from covenant_data.testing import assert_conforms

    def test_orders_export(tmp_path):
        df = build_orders()
        assert_conforms(df, "contracts/orders.yaml")

A failure shows the report — each rule, row and value — as the assertion
message.
"""

from . import check

__all__ = ["assert_conforms"]


def assert_conforms(data, contract, **kwargs):
    """Assert that ``data`` passes ``contract``; return the :class:`Report`.

    Takes the same arguments as :func:`covenant_data.check`.
    """
    report = check(data, contract, **kwargs)
    if not report.passed:
        raise AssertionError("data does not conform to the contract\n" + str(report))
    return report
