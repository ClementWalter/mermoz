"""Apple preflight fails before signing when the website-created app record is missing."""

import click
import pytest
import release


class AppleStub:
    def __init__(self, *, bundle=True, app=True):
        self.bundle, self.app = bundle, app
        self.registered = []

    def request(self, path, *, method="GET", data=None):
        if method == "POST":
            self.registered.append(data["data"]["attributes"]["identifier"])
            return {"data": {"id": "bundle"}}
        if path.startswith("bundleIds?"):
            return {"data": [{"id": "bundle"}] if self.bundle else []}
        if path.startswith("apps?"):
            return {"data": [{"id": "app"}] if self.app else []}
        raise AssertionError(path)


def test_an_existing_app_is_ready():
    assert release.ios_preflight(AppleStub()) == "app"


def test_a_missing_bundle_is_registered():
    api = AppleStub(bundle=False)
    release.ios_preflight(api)
    assert api.registered == [release.IOS_BUNDLE]


def test_a_missing_app_requires_the_apple_website():
    with pytest.raises(click.ClickException, match="Apple's API cannot create"):
        release.ios_preflight(AppleStub(app=False))


@pytest.mark.parametrize("publish,destination", [(True, "upload"), (False, "export")])
def test_exports_use_the_requested_destination(publish, destination):
    assert release.ios_export_options("TEAM", publish)["destination"] == destination


def test_exports_keep_the_locally_tested_build_number():
    assert release.ios_export_options("TEAM", True)["manageAppVersionAndBuildNumber"] is False
