"""Synthetic peer-copy fixture; not sourced from the reference library."""

from collections.abc import Iterable


def rolling_average(samples: Iterable[float], window_size: int) -> list[float]:
    """Return the simple moving average for each complete window."""
    if window_size <= 0:
        raise ValueError("window_size must be positive")

    window: list[float] = []
    running_total = 0.0
    averages: list[float] = []
    for sample in samples:
        value = float(sample)
        window.append(value)
        running_total += value
        if len(window) > window_size:
            running_total -= window.pop(0)
        if len(window) == window_size:
            averages.append(running_total / window_size)
    return averages


def threshold_crossings(values: Iterable[float], threshold: float) -> list[int]:
    """Return indexes where a sequence first moves from below to above a limit."""
    crossings: list[int] = []
    previous = None
    for index, value in enumerate(values):
        if previous is not None and previous < threshold <= value:
            crossings.append(index)
        previous = value
    return crossings
