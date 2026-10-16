"""Simulator selection uses the actual available inventory and fails on stale device ids."""

import build_app
import click
import pytest


@pytest.fixture
def devices():
    return {"devices": {"runtime": [
        {"name": "iPad", "udid": "tablet", "state": "Booted", "isAvailable": True},
        {"name": "iPhone A", "udid": "first", "state": "Shutdown", "isAvailable": True},
        {"name": "iPhone B", "udid": "running", "state": "Booted", "isAvailable": True},
        {"name": "iPhone C", "udid": "unavailable", "state": "Shutdown", "isAvailable": False},
    ]}}


def test_a_booted_iphone_wins(devices):
    assert build_app.simulator_id(devices) == "running"


def test_an_explicit_iphone_wins(devices):
    assert build_app.simulator_id(devices, "first") == "first"


@pytest.mark.parametrize("requested", ["missing", "unavailable", "tablet"])
def test_unusable_devices_fail(devices, requested):
    with pytest.raises(click.ClickException, match="No matching iPhone"):
        build_app.simulator_id(devices, requested)


def test_an_empty_inventory_fails():
    with pytest.raises(click.ClickException, match="No matching iPhone"):
        build_app.simulator_id({})
