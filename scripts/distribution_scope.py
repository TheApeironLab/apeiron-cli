"""Bind public distribution coverage to the applications offered by the wizard."""
import copy
import json
from pathlib import Path


GPU_HOST_ARCHIVE_PREFIXES = ('images/theapeironlab_gpustack_', 'images/gpustack_',
                             'images/vllm_', 'images/nvidia_k8s-device-plugin_')


def external_service_catalog(catalog):
    """Keep the existing GPUStack service integration without GPU host images."""
    result = copy.deepcopy(catalog)
    component = result['components'].get('gpustack')
    if component is None:
        return result
    removed = {image['file'] for image in component.get('images', [])}
    removed.update(path for path in component['files'] if path.startswith(GPU_HOST_ARCHIVE_PREFIXES))
    for name, other in result['components'].items():
        if name != 'gpustack' and removed.intersection(other['files']):
            raise ValueError('GPU host image unexpectedly shared with component: ' + name)
    component['images'] = []
    component['files'] = [path for path in component['files'] if path not in removed]
    result['files'] = [item for item in result['files'] if item['path'] not in removed]
    return result


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
    if components.get('gpustack', {}).get('images') or any(path.startswith(GPU_HOST_ARCHIVE_PREFIXES) for path in files):
        raise ValueError('GPUStack uses an existing external service; GPU host images must not be distributed')
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
