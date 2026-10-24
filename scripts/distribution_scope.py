"""Bind public distribution coverage to the applications offered by the wizard."""
import copy
import json
from pathlib import Path


GPU_HOST_ARCHIVE_PREFIXES = ('images/theapeironlab_gpustack_', 'images/gpustack_',
                             'images/vllm_', 'images/nvidia_k8s-device-plugin_')


def catalog_image_pins(components, registry):
    pins = {}
    for component in components.values():
        for image in component.get('images', []):
            source = registry + '/' + image['reference']
            target = source + '@' + image['digest']
            if source in pins and pins[source] != target:
                raise ValueError('conflicting catalog image identity: ' + source)
            pins[source] = target
    return pins


def platform_build_provenance(catalog, architecture):
    """Expose the selected platform recipe while retaining complete index evidence."""
    result = copy.deepcopy(catalog)
    platform = 'linux/' + architecture
    for component in result['components'].values():
        for image in component.get('images', []):
            provenance = image.get('buildProvenance')
            if not provenance or 'platformBuilds' not in provenance:
                continue
            builds = provenance['platformBuilds']
            selected = [build for build in builds if build.get('platform') == platform]
            if len(selected) != 1:
                raise ValueError('multiarchitecture provenance requires one build for ' + platform)
            build = selected[0]
            if build.get('source') != provenance.get('source'):
                raise ValueError('platform build source differs from index source')
            for name, value in (('recipe', build['recipe']), ('platform', platform)):
                if name in provenance and provenance[name] != value:
                    raise ValueError('conflicting platform build provenance: ' + name)
                provenance[name] = copy.deepcopy(value)
    return result


def application_releases(catalog):
    """The catalog's applications (Chentu applications.yaml) as {ID: releases}."""
    if not isinstance(catalog.get('applications'), list):
        raise ValueError('catalog has no application declaration; pin a newer Chentu')
    return {app['id']: app['releases'] for app in catalog['applications']}


def required_components(recipe, catalog):
    if recipe.get('applicationScope') != 'wizard':
        raise ValueError('public distribution requires the complete wizard application scope')
    return sorted(set(recipe['requiredComponents']) | {
        release for releases in application_releases(catalog).values() for release in releases})


def validate_catalog(catalog, required=None):
    required = required or sorted({release for releases in application_releases(catalog).values() for release in releases})
    components = catalog['components']
    files = {item['path'] for item in catalog['files']}
    if components.get('gpustack', {}).get('images') or any(path.startswith(GPU_HOST_ARCHIVE_PREFIXES) for path in files):
        raise ValueError('GPUStack uses an existing external service; GPU host images must not be distributed')
    for target in catalog.get('targets', {}).values():
        environment = target.get('environment', {})
        if 'imagePins' in environment and environment['imagePins'] != catalog_image_pins(
                components, environment.get('registry', '__REGISTRY__')):
            raise ValueError('catalog imagePins differ from selected image receipts')
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
