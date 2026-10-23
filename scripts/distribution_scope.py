"""Bind public distribution coverage to the applications offered by the wizard."""
import json
from pathlib import Path


def application_releases():
    apps = json.loads((Path(__file__).parents[1] / 'runtime/assets/apps.json').read_text())
    return {app['id']: app.get('releases', [app['id']]) for app in apps}


def required_components(recipe):
    if recipe.get('applicationScope') != 'wizard':
        raise ValueError('public distribution requires the complete wizard application scope')
    return sorted(set(recipe['requiredComponents']) | {
        release for releases in application_releases().values() for release in releases})


def validate_catalog(catalog, required=None):
    required = required or sorted({release for releases in application_releases().values() for release in releases})
    components = catalog['components']
    files = {item['path'] for item in catalog['files']}
    visited = set()

    def visit(name):
        if name in visited:
            return
        if name not in components:
            raise ValueError('wizard distribution missing component: ' + name)
        visited.add(name)
        component = components[name]
        for resource in component['files']:
            if resource not in files:
                raise ValueError('wizard distribution missing resource: ' + resource)
        for image in component.get('images', []):
            if image['file'] not in component['files']:
                raise ValueError('wizard image is not owned by its component: ' + name)
        for dependency in component['requires']:
            visit(dependency)

    for name in required:
        visit(name)
    return sorted(visited)
