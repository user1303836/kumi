"""Optional Control Surface entry point for the exact-build device extensions."""
import json
from pathlib import Path


def create_instance(c_instance):
    from _Framework.ControlSurface import ControlSurface
    from .api import install

    class DeviceTools(ControlSurface):
        def __init__(self, instance):
            super(DeviceTools, self).__init__(instance)
            self._native = None
            self.schedule_message(1, self._install)

        def _install(self):
            try:
                path = Path(__file__).with_name('config.json')
                config = json.loads(path.read_text()) if path.exists() else {}
                enabled = config.get('enable_writes', False)
                if type(enabled) is not bool:
                    raise ValueError('enable_writes must be a JSON boolean')
                self._native = install()
                self._native.enable(enabled)
                self.log_message('Willington Device Tools registered; writes ' +
                                 ('enabled' if enabled else 'disabled'))
            except Exception:
                import traceback
                if self._native is not None:
                    self._native.uninstall()
                self.log_message('Willington Device Tools unavailable: ' + traceback.format_exc())

        def disconnect(self):
            if self._native is not None:
                self._native.uninstall()
            super(DeviceTools, self).disconnect()

    return DeviceTools(c_instance)
