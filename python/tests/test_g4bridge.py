import argparse
import os
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))
g4bridge = pytest.importorskip('g4bridge')  # needs Windows (ws2_32 / bthprops)


def test_axis_centre_and_limits():
    assert g4bridge.axis(128) == 0
    assert g4bridge.axis(255) == 32766
    assert g4bridge.axis(0) == -32768


def test_axis_invert_clamps():
    assert g4bridge.axis(0, invert=True) == 32767
    assert g4bridge.axis(255, invert=True) == -32766


def test_parse_addr_roundtrip():
    value = g4bridge.parse_addr('AA:BB:CC:DD:EE:FF')
    assert value == 0xAABBCCDDEEFF
    assert g4bridge.fmt_addr(value) == 'AA:BB:CC:DD:EE:FF'


@pytest.mark.parametrize('text', ['', 'AA:BB', 'ZZ:BB:CC:DD:EE:FF', 'AA:BB:CC:DD:EE:FF:00'])
def test_parse_addr_rejects_bad_input(text):
    with pytest.raises(argparse.ArgumentTypeError):
        g4bridge.parse_addr(text)


def test_hat_covers_all_eight_directions():
    assert sorted(g4bridge.HAT) == list(range(1, 9))
    assert g4bridge.HAT[1] == (0, 1) and g4bridge.HAT[5] == (0, -1)
