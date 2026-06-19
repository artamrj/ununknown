export type SettingsSectionProps = {
  settings: any;
  visible: (text: string) => boolean;
  set: (key: string, value: any) => void;
  nested: (section: string, key: string, value: any) => void;
  reset: (section?: string) => void;
};

export type FilesAndPathsSettingsProps = SettingsSectionProps & {
  pathPreview: {
    isLoading: boolean;
    data?: {
      examples?: Array<{
        label: string;
        template: string;
        path?: string;
        errors?: string[];
        warnings?: string[];
      }>;
    };
  };
};
